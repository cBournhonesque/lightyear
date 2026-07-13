//! Scenario 2: `CatchUpMode::StateBasedCatchUp` with BEI `Axis2D` inputs,
//! random per-tick movements and tight collisions.
//!
//! Two clients connect. Late-join delays the bundled catch-up snapshot until
//! the server has received inputs from every connected client. Each client
//! receives the bundle in one replicon update and the plugin fires a single
//! forced rollback to reconcile.
//!
//! Both clients then drive their player with randomised Axis2D inputs
//! switching every tick, inside a small box that forces frequent
//! player↔player and player↔ball collisions. We assert that the final
//! Position on each client matches the server bit-perfectly (via Avian's
//! `enhanced-determinism` feature + our catch-up machinery).

use crate::client_server::deterministic::protocol::{
    DetBallMarker, DetBuffer, DetMovement, DetPlayerActivationTick, DetPlayerId, DetProtocolPlugin,
    DetWallMarker, Player,
};
use crate::client_server::deterministic::stepper::{
    DetStepper, spawn_local_action_on_client, spawn_player_on_server,
};
use approx::assert_relative_eq;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{
    Action, ActionMock, ActionOf, ActionValue, MockSpan, TriggerState,
};
use lightyear::input::bei::input_message::ActionsSnapshot;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::CatchUpSnapshotReady;
use lightyear_messages::MessageManager;
use lightyear_prediction::rollback::CatchUpGated;
use std::collections::HashMap;
use test_log::test;

/// Resource on each client driving a deterministic pseudo-random walk
/// for its BEI action.
#[derive(Resource, Clone)]
struct RandomDrive {
    seed: u64,
    /// Keep the action still for this many ticks at the start so catch-up
    /// machinery can settle (matches "50 ticks then start moving" request).
    warmup_ticks: u32,
    direction: Option<Vec2>,
}

#[derive(Resource, Default)]
struct PositionSamples(HashMap<(u32, PeerId), Position>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MotionSubject {
    Player(PeerId),
    Ball,
}

#[derive(Clone, Copy, Debug)]
struct MotionSample {
    position: Position,
    velocity: LinearVelocity,
}

#[derive(Resource, Default)]
struct MotionSamples(HashMap<(u32, MotionSubject), MotionSample>);

#[derive(Resource, Default)]
struct InputSamples(HashMap<(u32, PeerId), Option<ActionsSnapshot>>);

#[derive(Clone, Copy, Debug)]
struct ExecutionSample {
    tick: Tick,
    rollback: Option<Rollback>,
    rollback_start: Option<Tick>,
}

#[derive(Resource, Default)]
struct ExecutionSamples(Vec<ExecutionSample>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleStage {
    PrePhysics,
    PostPhysics,
}

#[derive(Clone, Copy, Debug)]
struct StageMotionSample {
    tick: Tick,
    stage: SampleStage,
    rollback: Option<Rollback>,
    rollback_start: Option<Tick>,
    subject: MotionSubject,
    motion: MotionSample,
}

#[derive(Resource, Default)]
struct StageMotionSamples(Vec<StageMotionSample>);

#[derive(Clone, Debug)]
struct ContactPairSample {
    collider1: String,
    collider2: String,
    body1: Option<String>,
    body2: Option<String>,
    touching: bool,
    normals: Vec<Vec2>,
    max_normal_impulse: f32,
}

#[derive(Clone, Debug)]
struct ContactSample {
    tick: Tick,
    stage: SampleStage,
    rollback: Option<Rollback>,
    rollback_start: Option<Tick>,
    pairs: Vec<ContactPairSample>,
}

#[derive(Resource, Default)]
struct ContactSamples(Vec<ContactSample>);

#[derive(Clone, Debug)]
struct CatchUpActivationTrace {
    client_label: String,
    local_tick: Tick,
    reference_tick: Tick,
    positions: Vec<(PeerId, Position)>,
    buffers: Vec<(
        PeerId,
        Option<Tick>,
        Option<Tick>,
        Option<Tick>,
        Option<ActionsSnapshot>,
        Vec<(Tick, Option<ActionsSnapshot>)>,
    )>,
}

#[derive(Resource, Default)]
struct CatchUpTrace(Vec<CatchUpActivationTrace>);

impl RandomDrive {
    fn new(seed: u64, warmup_ticks: u32) -> Self {
        Self {
            seed: seed.wrapping_add(0x9E3779B97F4A7C15),
            warmup_ticks,
            direction: None,
        }
    }

    fn fixed(direction: Vec2, warmup_ticks: u32) -> Self {
        Self {
            seed: 0,
            warmup_ticks,
            direction: Some(direction),
        }
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Each tick, write a random unit vector into the client's `ActionMock`
/// so BEI reports a non-zero `Axis2D` via `Fire<DetMovement>`.
fn drive_random_input(
    random: Res<RandomDrive>,
    timeline: Res<LocalTimeline>,
    mut actions: Query<&mut ActionMock, With<Action<DetMovement>>>,
) {
    let tick = timeline.tick();
    let dir = if tick.0 < random.warmup_ticks {
        Vec2::ZERO
    } else if let Some(direction) = random.direction {
        direction
    } else {
        // Pick one of 4 cardinal directions pseudo-randomly.
        let mut state = random
            .seed
            .wrapping_add((tick.0 as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
        let r = splitmix64(&mut state);
        match r & 0b11 {
            0 => Vec2::X,
            1 => -Vec2::X,
            2 => Vec2::Y,
            _ => -Vec2::Y,
        }
    };
    for mut mock in &mut actions {
        mock.state = TriggerState::Fired;
        mock.value = ActionValue::Axis2D(dir);
        mock.span = MockSpan::Manual;
        mock.enabled = true;
    }
}

fn activate_physics_when_bundle_lands(
    trigger: On<CatchUpSnapshotReady>,
    mut commands: Commands,
    pending: Query<
        (Entity, &DetPlayerId),
        (
            With<CatchUpGated>,
            With<DetPlayerId>,
            Without<DeterministicPredicted>,
        ),
    >,
    timeline: Res<LocalTimeline>,
    all_players: Query<(
        Entity,
        &DetPlayerId,
        Option<&DetPlayerActivationTick>,
        Option<&Position>,
    )>,
    mut input_buffers: Query<(&ActionOf<Player>, Option<&mut DetBuffer>)>,
    mut trace: ResMut<CatchUpTrace>,
) {
    use crate::client_server::deterministic::protocol::DetPhysicsBundle;

    let mut ready = Vec::new();
    for (entity, id) in &pending {
        ready.push((entity, id.0));
    }
    if ready.is_empty() {
        return;
    }
    let event = trigger.event();
    record_catchup_activation_trace(
        &mut trace,
        timeline.tick(),
        event.server_tick,
        &all_players,
        &mut input_buffers,
    );
    // Avian can depend on insertion order for internal proxy/body ordering.
    // The deterministic example activates players in this game-defined order;
    // keep the regression test on the same path.
    ready.sort_by_key(|(_, peer_id)| peer_id.to_bits());

    for (entity, _id) in ready {
        commands.entity(entity).insert((
            DetPhysicsBundle::player(),
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
        ));
    }
}

fn record_catchup_activation_trace(
    trace: &mut CatchUpTrace,
    local_tick: Tick,
    reference_tick: Tick,
    players: &Query<(
        Entity,
        &DetPlayerId,
        Option<&DetPlayerActivationTick>,
        Option<&Position>,
    )>,
    input_buffers: &mut Query<(&ActionOf<Player>, Option<&mut DetBuffer>)>,
) {
    use bevy::ecs::relationship::Relationship;

    let mut buffers = Vec::new();
    let mut positions = Vec::new();
    for (player, player_id, _, position) in players.iter() {
        if let Some(position) = position {
            positions.push((player_id.0, *position));
        }
        let input = input_buffers.iter_mut().find_map(|(action_of, buffer)| {
            if action_of.get() != player {
                return None;
            }
            let buffer = buffer?;
            let start = reference_tick - 3;
            let end = reference_tick + 8;
            let mut window = Vec::new();
            let mut tick = start;
            while tick <= end {
                window.push((tick, buffer.get(tick).cloned()));
                tick = tick + 1;
            }
            Some((
                buffer.start_tick,
                buffer.end_tick(),
                buffer.last_remote_tick,
                buffer.get(reference_tick).cloned(),
                window,
            ))
        });
        if let Some((start, end, last_remote, value, window)) = input {
            buffers.push((player_id.0, start, end, last_remote, value, window));
        } else {
            buffers.push((player_id.0, None, None, None, None, Vec::new()));
        }
    }
    trace.0.push(CatchUpActivationTrace {
        client_label: "client".to_string(),
        local_tick,
        reference_tick,
        positions,
        buffers,
    });
}

/// Wire up the physics-activation and random-drive systems on each client.
fn configure_stepper(stepper: &mut DetStepper, warmup_ticks: u32) {
    add_position_samples(&mut stepper.server_app);
    for (i, client_app) in stepper.client_apps.iter_mut().enumerate() {
        configure_client_app(client_app, i as u64 + 1, warmup_ticks);
    }
}

fn configure_client_app(client_app: &mut App, seed: u64, warmup_ticks: u32) {
    configure_client_app_with_drive(client_app, RandomDrive::new(seed, warmup_ticks));
}

fn configure_client_app_with_drive(client_app: &mut App, drive: RandomDrive) {
    client_app.insert_resource(drive);
    client_app.init_resource::<CatchUpTrace>();
    add_position_samples(client_app);
    client_app.add_observer(activate_physics_when_bundle_lands);
    client_app.add_systems(FixedPreUpdate, drive_random_input);
}

fn configure_stepper_with_fixed_drives(
    stepper: &mut DetStepper,
    warmup_ticks: u32,
    directions: &[Vec2],
) {
    add_position_samples(&mut stepper.server_app);
    for (client_app, direction) in stepper
        .client_apps
        .iter_mut()
        .zip(directions.iter().copied())
    {
        configure_client_app_with_drive(client_app, RandomDrive::fixed(direction, warmup_ticks));
    }
}

fn add_position_samples(app: &mut App) {
    app.init_resource::<PositionSamples>();
    app.init_resource::<MotionSamples>();
    app.init_resource::<InputSamples>();
    app.init_resource::<ExecutionSamples>();
    app.init_resource::<StageMotionSamples>();
    app.init_resource::<ContactSamples>();
    app.add_systems(
        FixedPostUpdate,
        (
            sample_pre_physics_state.before(PhysicsSystems::StepSimulation),
            sample_post_physics_state.after(PhysicsSystems::StepSimulation),
        ),
    );
    app.add_systems(FixedLast, sample_positions);
}

fn sample_pre_physics_state(
    timeline: Res<LocalTimeline>,
    motion_samples: ResMut<StageMotionSamples>,
    contact_samples: ResMut<ContactSamples>,
    player_motion: Query<(&DetPlayerId, &Position, &LinearVelocity), With<DeterministicPredicted>>,
    ball_motion: Query<
        (&Position, &LinearVelocity),
        (With<DetBallMarker>, With<DeterministicPredicted>),
    >,
    contact_graph: Option<Res<ContactGraph>>,
    players: Query<(Entity, &DetPlayerId)>,
    balls: Query<Entity, With<DetBallMarker>>,
    walls: Query<Entity, With<DetWallMarker>>,
    rollback_markers: Query<&Rollback>,
    prediction_managers: Query<&PredictionManager>,
) {
    sample_physics_state(
        SampleStage::PrePhysics,
        timeline,
        motion_samples,
        contact_samples,
        player_motion,
        ball_motion,
        contact_graph,
        players,
        balls,
        walls,
        rollback_markers,
        prediction_managers,
    );
}

fn sample_post_physics_state(
    timeline: Res<LocalTimeline>,
    motion_samples: ResMut<StageMotionSamples>,
    contact_samples: ResMut<ContactSamples>,
    player_motion: Query<(&DetPlayerId, &Position, &LinearVelocity), With<DeterministicPredicted>>,
    ball_motion: Query<
        (&Position, &LinearVelocity),
        (With<DetBallMarker>, With<DeterministicPredicted>),
    >,
    contact_graph: Option<Res<ContactGraph>>,
    players: Query<(Entity, &DetPlayerId)>,
    balls: Query<Entity, With<DetBallMarker>>,
    walls: Query<Entity, With<DetWallMarker>>,
    rollback_markers: Query<&Rollback>,
    prediction_managers: Query<&PredictionManager>,
) {
    sample_physics_state(
        SampleStage::PostPhysics,
        timeline,
        motion_samples,
        contact_samples,
        player_motion,
        ball_motion,
        contact_graph,
        players,
        balls,
        walls,
        rollback_markers,
        prediction_managers,
    );
}

#[allow(clippy::too_many_arguments)]
fn sample_physics_state(
    stage: SampleStage,
    timeline: Res<LocalTimeline>,
    mut motion_samples: ResMut<StageMotionSamples>,
    mut contact_samples: ResMut<ContactSamples>,
    player_motion: Query<(&DetPlayerId, &Position, &LinearVelocity), With<DeterministicPredicted>>,
    ball_motion: Query<
        (&Position, &LinearVelocity),
        (With<DetBallMarker>, With<DeterministicPredicted>),
    >,
    contact_graph: Option<Res<ContactGraph>>,
    players: Query<(Entity, &DetPlayerId)>,
    balls: Query<Entity, With<DetBallMarker>>,
    walls: Query<Entity, With<DetWallMarker>>,
    rollback_markers: Query<&Rollback>,
    prediction_managers: Query<&PredictionManager>,
) {
    let tick = timeline.tick();
    let rollback = rollback_markers.iter().next().copied();
    let rollback_start = prediction_managers
        .iter()
        .next()
        .and_then(PredictionManager::get_rollback_start_tick);
    for (id, position, velocity) in &player_motion {
        motion_samples.0.push(StageMotionSample {
            tick,
            stage,
            rollback,
            rollback_start,
            subject: MotionSubject::Player(id.0),
            motion: MotionSample {
                position: *position,
                velocity: *velocity,
            },
        });
    }
    for (position, velocity) in &ball_motion {
        motion_samples.0.push(StageMotionSample {
            tick,
            stage,
            rollback,
            rollback_start,
            subject: MotionSubject::Ball,
            motion: MotionSample {
                position: *position,
                velocity: *velocity,
            },
        });
    }
    let Some(contact_graph) = contact_graph else {
        return;
    };
    let mut labels = HashMap::<Entity, String>::new();
    for (entity, player) in &players {
        labels.insert(entity, format!("player:{:?}", player.0));
    }
    for entity in &balls {
        labels.insert(entity, "ball".to_string());
    }
    for entity in &walls {
        labels.insert(entity, "wall".to_string());
    }
    let pairs = contact_graph
        .iter_active()
        .map(|pair| ContactPairSample {
            collider1: entity_label(pair.collider1, &labels),
            collider2: entity_label(pair.collider2, &labels),
            body1: pair.body1.map(|entity| entity_label(entity, &labels)),
            body2: pair.body2.map(|entity| entity_label(entity, &labels)),
            touching: pair.is_touching(),
            normals: pair
                .manifolds
                .iter()
                .map(|manifold| manifold.normal)
                .collect(),
            max_normal_impulse: pair.max_normal_impulse_magnitude(),
        })
        .collect();
    contact_samples.0.push(ContactSample {
        tick,
        stage,
        rollback,
        rollback_start,
        pairs,
    });
}

fn entity_label(entity: Entity, labels: &HashMap<Entity, String>) -> String {
    labels
        .get(&entity)
        .cloned()
        .unwrap_or_else(|| format!("{entity:?}"))
}

fn sample_positions(
    timeline: Res<LocalTimeline>,
    mut samples: ResMut<PositionSamples>,
    mut motion_samples: ResMut<MotionSamples>,
    mut input_samples: ResMut<InputSamples>,
    mut execution_samples: ResMut<ExecutionSamples>,
    players: Query<(&DetPlayerId, &Position), With<DeterministicPredicted>>,
    player_motion: Query<(&DetPlayerId, &Position, &LinearVelocity), With<DeterministicPredicted>>,
    ball_motion: Query<
        (&Position, &LinearVelocity),
        (With<DetBallMarker>, With<DeterministicPredicted>),
    >,
    action_players: Query<(Entity, &DetPlayerId)>,
    action_buffers: Query<(&ActionOf<Player>, &DetBuffer)>,
    rollback_markers: Query<&Rollback>,
    prediction_managers: Query<&PredictionManager>,
) {
    use bevy::ecs::relationship::Relationship;

    let tick = timeline.tick().0;
    let rollback = rollback_markers.iter().next().copied();
    let rollback_start = prediction_managers
        .iter()
        .next()
        .and_then(PredictionManager::get_rollback_start_tick);
    execution_samples.0.push(ExecutionSample {
        tick: Tick(tick),
        rollback,
        rollback_start,
    });
    for (id, position) in &players {
        samples.0.insert((tick, id.0), *position);
    }
    for (id, position, velocity) in &player_motion {
        motion_samples.0.insert(
            (tick, MotionSubject::Player(id.0)),
            MotionSample {
                position: *position,
                velocity: *velocity,
            },
        );
    }
    for (position, velocity) in &ball_motion {
        motion_samples.0.insert(
            (tick, MotionSubject::Ball),
            MotionSample {
                position: *position,
                velocity: *velocity,
            },
        );
    }
    for (player, player_id) in &action_players {
        let input = action_buffers.iter().find_map(|(action_of, buffer)| {
            (action_of.get() == player).then(|| buffer.get(Tick(tick)).cloned())
        });
        input_samples.0.insert((tick, player_id.0), input.flatten());
    }
}

fn fixed_position_at(world: &World, player_id: PeerId, tick: Tick) -> Position {
    world
        .resource::<PositionSamples>()
        .0
        .get(&(tick.0, player_id))
        .copied()
        .unwrap_or_else(|| {
            let local_tick = world.resource::<LocalTimeline>().tick();
            panic!(
                "cannot compare fixed Position for player {:?} at tick {:?}; app is at local tick {:?}",
                player_id, tick, local_tick
            )
        })
}

fn motion_sample_at(world: &World, subject: MotionSubject, tick: Tick) -> MotionSample {
    world
        .resource::<MotionSamples>()
        .0
        .get(&(tick.0, subject))
        .copied()
        .unwrap_or_else(|| {
            let local_tick = world.resource::<LocalTimeline>().tick();
            panic!(
                "cannot compare deterministic motion for {subject:?} at tick {tick:?}; app is at local tick {local_tick:?}"
            )
        })
}

fn wait_for_client_mapping(
    stepper: &mut DetStepper,
    client_id: usize,
    server_entity: Entity,
) -> Entity {
    for _ in 0..120 {
        if let Some(local) = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
        {
            return local;
        }
        stepper.frame_step(1);
    }
    panic!("client {client_id} did not receive server entity {server_entity:?}");
}

fn spawn_local_action_after_mapping(
    stepper: &mut DetStepper,
    client_id: usize,
    server_player: Entity,
    peer: PeerId,
) {
    let local_player = wait_for_client_mapping(stepper, client_id, server_player);
    spawn_local_action_on_client(stepper.client_app(client_id), local_player, peer);
}

fn despawn_server_player_and_action(stepper: &mut DetStepper, player: Entity) {
    use crate::client_server::deterministic::protocol::Player;
    use bevy::ecs::relationship::Relationship;

    let mut query = stepper
        .server_app
        .world_mut()
        .query::<(Entity, &ActionOf<Player>)>();
    let actions: Vec<Entity> = query
        .iter(stepper.server_app.world())
        .filter_map(|(entity, action_of)| (action_of.get() == player).then_some(entity))
        .collect();
    for action in actions {
        stepper.server_app.world_mut().despawn(action);
    }
    if stepper.server_app.world().get_entity(player).is_ok() {
        stepper.server_app.world_mut().despawn(player);
    }
}

fn assert_clean_player_entities(world: &mut World, label: &str, expected_peers: &[PeerId]) {
    let mut players = world.query::<(Entity, &DetPlayerId)>();
    let rows = players
        .iter(world)
        .map(|(entity, player_id)| (entity, player_id.0))
        .collect::<Vec<_>>();

    for peer in expected_peers {
        let matches = rows
            .iter()
            .filter(|(_, player_id)| player_id == peer)
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one player for {peer:?} in {label}; rows={rows:?}"
        );
    }

    let unexpected = rows
        .iter()
        .filter(|(_, player_id)| !expected_peers.contains(player_id))
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "unexpected player entities in {label}; expected={expected_peers:?} rows={rows:?}"
    );
}

fn assert_clean_stepper_player_entities(stepper: &mut DetStepper, expected_peers: &[PeerId]) {
    assert_clean_player_entities(stepper.server_app.world_mut(), "server", expected_peers);
    for client_id in 0..stepper.client_apps.len() {
        let label = format!("client {client_id}");
        assert_clean_player_entities(
            stepper.client_app(client_id).world_mut(),
            &label,
            expected_peers,
        );
    }
}

fn assert_server_entity_unmapped(stepper: &mut DetStepper, server_entity: Entity) {
    for client_id in 0..stepper.client_apps.len() {
        assert!(
            stepper
                .client(client_id)
                .get::<MessageManager>()
                .unwrap()
                .entity_mapper
                .get_local(server_entity)
                .is_none(),
            "client {client_id} still maps disconnected server entity {server_entity:?}"
        );
    }
}

fn assert_single_ball_entity(world: &mut World, label: &str) -> Entity {
    let mut balls = world.query_filtered::<(
        Entity,
        Has<Position>,
        Has<LinearVelocity>,
        Has<DeterministicPredicted>,
    ), With<DetBallMarker>>();
    let rows = balls.iter(world).collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one deterministic ball in {label}; rows={rows:?}"
    );
    let (entity, has_position, has_velocity, deterministic) = rows[0];
    assert!(
        has_position && has_velocity && deterministic,
        "ball in {label} should have live deterministic physics state; rows={rows:?}"
    );
    entity
}

fn assert_clean_stepper_ball_entities(stepper: &mut DetStepper) {
    let server_ball = assert_single_ball_entity(stepper.server_app.world_mut(), "server");
    for client_id in 0..stepper.client_apps.len() {
        let label = format!("client {client_id}");
        let client_ball =
            assert_single_ball_entity(stepper.client_app(client_id).world_mut(), &label);
        assert_eq!(
            stepper
                .client(client_id)
                .get::<MessageManager>()
                .unwrap()
                .entity_mapper
                .get_local(server_ball),
            Some(client_ball),
            "client {client_id} should map the server ball entity {server_ball:?}"
        );
    }
}

fn assert_no_awaiting_catchup(world: &mut World, label: &str) {
    let mut awaiting = world
        .query_filtered::<(Entity, Option<&DetPlayerId>, Has<DetBallMarker>), With<CatchUpGated>>();
    let rows = awaiting
        .iter(world)
        .map(|(entity, player_id, ball)| (entity, player_id.map(|id| id.0), ball))
        .collect::<Vec<_>>();
    assert!(
        rows.is_empty(),
        "expected no CatchupGated entities in {label}; rows={rows:?}"
    );
}

fn assert_stepper_catchup_complete(stepper: &mut DetStepper) {
    for client_id in 0..stepper.client_apps.len() {
        let label = format!("client {client_id}");
        assert_no_awaiting_catchup(stepper.client_app(client_id).world_mut(), &label);
    }
}

fn compare_players_to_server(
    stepper: &mut DetStepper,
    server_players: &[Entity],
    peers: &[PeerId],
) {
    let latest_tick = stepper
        .client_apps
        .iter()
        .enumerate()
        .map(|(client_id, _)| stepper.client_tick(client_id))
        .fold(stepper.server_tick(), |min_tick, tick| {
            if tick.0 < min_tick.0 { tick } else { min_tick }
        });
    let compare_tick = latest_common_sample_tick(stepper, peers, latest_tick);

    for (server_player, peer) in server_players.iter().copied().zip(peers.iter().copied()) {
        let server_pos = fixed_position_at(stepper.server_app.world(), peer, compare_tick);
        for client_id in 0..stepper.client_apps.len() {
            let _client_player = stepper
                .client(client_id)
                .get::<MessageManager>()
                .unwrap()
                .entity_mapper
                .get_local(server_player)
                .expect("client missing player entity");
            let client_pos =
                fixed_position_at(stepper.client_app(client_id).world(), peer, compare_tick);
            info!(
                client_id,
                ?peer,
                server_tick = ?stepper.server_tick(),
                ?compare_tick,
                ?server_pos,
                ?client_pos,
                "comparing deterministic fixed positions"
            );
            assert_relative_eq!(client_pos.x, server_pos.x, epsilon = 0.01);
            assert_relative_eq!(client_pos.y, server_pos.y, epsilon = 0.01);
        }
    }
}

fn compare_deterministic_motion_to_server(stepper: &mut DetStepper, peers: &[PeerId]) {
    let subjects = peers
        .iter()
        .copied()
        .map(MotionSubject::Player)
        .chain([MotionSubject::Ball])
        .collect::<Vec<_>>();
    let latest_tick = stepper
        .client_apps
        .iter()
        .enumerate()
        .map(|(client_id, _)| stepper.client_tick(client_id))
        .fold(stepper.server_tick(), |min_tick, tick| {
            if tick.0 < min_tick.0 { tick } else { min_tick }
        });
    let compare_tick = latest_common_motion_sample_tick(stepper, &subjects, latest_tick);

    for subject in subjects {
        let server_sample = motion_sample_at(stepper.server_app.world(), subject, compare_tick);
        for client_id in 0..stepper.client_apps.len() {
            let client_sample =
                motion_sample_at(stepper.client_app(client_id).world(), subject, compare_tick);
            info!(
                client_id,
                ?subject,
                server_tick = ?stepper.server_tick(),
                ?compare_tick,
                ?server_sample,
                ?client_sample,
                "comparing deterministic motion"
            );
            assert_relative_eq!(
                client_sample.position.0.x,
                server_sample.position.0.x,
                epsilon = 0.01
            );
            assert_relative_eq!(
                client_sample.position.0.y,
                server_sample.position.0.y,
                epsilon = 0.01
            );
            assert_relative_eq!(
                client_sample.velocity.0.x,
                server_sample.velocity.0.x,
                epsilon = 0.01
            );
            assert_relative_eq!(
                client_sample.velocity.0.y,
                server_sample.velocity.0.y,
                epsilon = 0.01
            );
        }
    }
}

fn dump_catchup_trace(stepper: &mut DetStepper, peers: &[PeerId]) {
    let server_samples = stepper
        .server_app
        .world()
        .resource::<PositionSamples>()
        .0
        .clone();
    let server_motion_samples = stepper
        .server_app
        .world()
        .resource::<MotionSamples>()
        .0
        .clone();
    let server_input_samples = stepper
        .server_app
        .world()
        .resource::<InputSamples>()
        .0
        .clone();
    let server_execution_samples = stepper
        .server_app
        .world()
        .resource::<ExecutionSamples>()
        .0
        .clone();
    let server_stage_motion_samples = stepper
        .server_app
        .world()
        .resource::<StageMotionSamples>()
        .0
        .clone();
    let server_contact_samples = stepper
        .server_app
        .world()
        .resource::<ContactSamples>()
        .0
        .clone();
    let client_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<PositionSamples>().0.clone())
        .collect::<Vec<_>>();
    let client_motion_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<MotionSamples>().0.clone())
        .collect::<Vec<_>>();
    let client_input_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<InputSamples>().0.clone())
        .collect::<Vec<_>>();
    let client_execution_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<ExecutionSamples>().0.clone())
        .collect::<Vec<_>>();
    let client_stage_motion_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<StageMotionSamples>().0.clone())
        .collect::<Vec<_>>();
    let client_contact_samples = stepper
        .client_apps
        .iter()
        .map(|app| app.world().resource::<ContactSamples>().0.clone())
        .collect::<Vec<_>>();
    for client_id in 0..stepper.client_apps.len() {
        let trace = stepper
            .client_app(client_id)
            .world()
            .resource::<CatchUpTrace>()
            .0
            .clone();
        for (activation_idx, activation) in trace.iter().enumerate() {
            if activation_idx + 1 != trace.len() {
                continue;
            }
            println!(
                "catchup trace client={client_id} activation={activation_idx} local_tick={:?} reference_tick={:?}",
                activation.local_tick, activation.reference_tick
            );
            println!(
                "  snapshot positions on activation: {:?}",
                activation.positions
            );
            for (peer, start, end, last_remote, value, window) in &activation.buffers {
                println!(
                    "  buffer peer={peer:?} start={start:?} end={end:?} last_remote={last_remote:?} input_at_ref={value:?}"
                );
                println!("  buffer_window peer={peer:?} {window:?}");
            }
            let start = activation.reference_tick - 3;
            let end = activation.reference_tick + 8;
            dump_input_window(stepper.server_app.world_mut(), "server", start.0..=end.0);
            dump_input_window(
                stepper.client_app(client_id).world_mut(),
                &format!("client{client_id}"),
                start.0..=end.0,
            );
            let mut tick = start;
            while tick <= end {
                let mut row = Vec::new();
                for peer in peers {
                    let server = server_samples.get(&(tick.0, *peer)).copied();
                    let client = client_samples
                        .get(client_id)
                        .and_then(|samples| samples.get(&(tick.0, *peer)).copied());
                    row.push((*peer, server, client));
                }
                println!("  samples tick={tick:?} {row:?}");
                let mut motion_row = Vec::new();
                for subject in peers
                    .iter()
                    .copied()
                    .map(MotionSubject::Player)
                    .chain([MotionSubject::Ball])
                {
                    let server = server_motion_samples
                        .get(&(tick.0, subject.clone()))
                        .copied();
                    let client = client_motion_samples
                        .get(client_id)
                        .and_then(|samples| samples.get(&(tick.0, subject.clone())).copied());
                    motion_row.push((subject, server, client));
                }
                println!("  motion tick={tick:?} {motion_row:?}");
                tick = tick + 1;
            }
        }
    }
    let mut first_mismatches = Vec::new();
    for client_id in 0..stepper.client_apps.len() {
        for peer in peers {
            let first_mismatch = (0..=stepper.server_tick().0).find_map(|tick| {
                let tick = Tick(tick);
                let server = server_motion_samples.get(&(tick.0, MotionSubject::Player(*peer)))?;
                let client = client_motion_samples
                    .get(client_id)?
                    .get(&(tick.0, MotionSubject::Player(*peer)))?;
                let pos_delta = (server.position.0 - client.position.0).length();
                let vel_delta = (server.velocity.0 - client.velocity.0).length();
                (pos_delta > 0.01 || vel_delta > 0.01)
                    .then_some((tick, *server, *client, pos_delta, vel_delta))
            });
            println!("first mismatch client={client_id} peer={peer:?}: {first_mismatch:?}");
            if let Some((tick, ..)) = first_mismatch {
                first_mismatches.push((client_id, *peer, tick));
            }
        }
    }
    for (client_id, peer, tick) in first_mismatches {
        let start = tick.0.saturating_sub(5);
        let end = tick.0 + 5;
        println!("mismatch window client={client_id} peer={peer:?} ticks={start}..={end}");
        dump_player_entities(stepper.server_app.world_mut(), "server");
        dump_player_entities(
            stepper.client_app(client_id).world_mut(),
            &format!("client{client_id}"),
        );
        println!(
            "  execution server {:?}",
            execution_window(&server_execution_samples, start..=end)
        );
        println!(
            "  execution client{client_id} {:?}",
            execution_window(&client_execution_samples[client_id], start..=end)
        );
        let boundary_start = tick.0.saturating_sub(1);
        let boundary_end = tick.0;
        println!(
            "  stage_motion server {:?}",
            stage_motion_window(&server_stage_motion_samples, boundary_start..=boundary_end)
        );
        println!(
            "  stage_motion client{client_id} {:?}",
            stage_motion_window(
                &client_stage_motion_samples[client_id],
                boundary_start..=boundary_end
            )
        );
        println!(
            "  contacts server {:?}",
            contact_window(&server_contact_samples, boundary_start..=boundary_end)
        );
        println!(
            "  contacts client{client_id} {:?}",
            contact_window(
                &client_contact_samples[client_id],
                boundary_start..=boundary_end
            )
        );
        dump_input_window(stepper.server_app.world_mut(), "server", start..=end);
        dump_input_window(
            stepper.client_app(client_id).world_mut(),
            &format!("client{client_id}"),
            start..=end,
        );
        for sample_tick in start..=end {
            let tick = Tick(sample_tick);
            let mut input_row = Vec::new();
            for peer in peers {
                let server = server_input_samples.get(&(tick.0, *peer)).cloned();
                let client = client_input_samples
                    .get(client_id)
                    .and_then(|samples| samples.get(&(tick.0, *peer)).cloned());
                input_row.push((*peer, server, client));
            }
            println!("  mismatch input tick={tick:?} {input_row:?}");
            let mut motion_row = Vec::new();
            for subject in peers
                .iter()
                .copied()
                .map(MotionSubject::Player)
                .chain([MotionSubject::Ball])
            {
                let server = server_motion_samples
                    .get(&(tick.0, subject.clone()))
                    .copied();
                let client = client_motion_samples
                    .get(client_id)
                    .and_then(|samples| samples.get(&(tick.0, subject.clone())).copied());
                motion_row.push((subject, server, client));
            }
            println!("  mismatch motion tick={tick:?} {motion_row:?}");
        }
    }
}

fn dump_player_entities(world: &mut World, label: &str) {
    let mut players = world.query::<(Entity, &DetPlayerId, Has<DeterministicPredicted>)>();
    let rows = players
        .iter(world)
        .map(|(entity, player_id, deterministic)| (entity, player_id.0, deterministic))
        .collect::<Vec<_>>();
    println!("  player_entities label={label} {rows:?}");
}

fn stage_motion_window(
    samples: &[StageMotionSample],
    ticks: std::ops::RangeInclusive<u32>,
) -> Vec<StageMotionSample> {
    samples
        .iter()
        .copied()
        .filter(|sample| ticks.contains(&sample.tick.0))
        .collect()
}

fn contact_window(
    samples: &[ContactSample],
    ticks: std::ops::RangeInclusive<u32>,
) -> Vec<ContactSample> {
    samples
        .iter()
        .filter(|sample| ticks.contains(&sample.tick.0))
        .cloned()
        .collect()
}

fn execution_window(
    samples: &[ExecutionSample],
    ticks: std::ops::RangeInclusive<u32>,
) -> Vec<ExecutionSample> {
    samples
        .iter()
        .copied()
        .filter(|sample| ticks.contains(&sample.tick.0))
        .collect()
}

fn dump_input_window(world: &mut World, label: &str, ticks: std::ops::RangeInclusive<u32>) {
    use bevy::ecs::relationship::Relationship;

    let mut players = world.query::<(Entity, &DetPlayerId, Option<&DetPlayerActivationTick>)>();
    let player_rows = players
        .iter(world)
        .map(|(entity, player_id, activation_tick)| (entity, player_id.0, activation_tick.copied()))
        .collect::<Vec<_>>();
    let mut actions = world.query::<(&ActionOf<Player>, &DetBuffer)>();
    for (player, player_id, activation_tick) in player_rows {
        let row = actions.iter(world).find_map(|(action_of, buffer)| {
            (action_of.get() == player).then(|| {
                let values = ticks
                    .clone()
                    .map(|tick| (Tick(tick), buffer.get(Tick(tick)).cloned()))
                    .collect::<Vec<_>>();
                (
                    buffer.start_tick,
                    buffer.end_tick(),
                    buffer.last_remote_tick,
                    values,
                )
            })
        });
        println!(
            "  input_window label={label} peer={player_id:?} activation_tick={activation_tick:?} row={row:?}"
        );
    }
}

fn latest_common_sample_tick(
    stepper: &mut DetStepper,
    peers: &[PeerId],
    latest_tick: Tick,
) -> Tick {
    let mut tick = latest_tick;
    for _ in 0..300 {
        if has_position_samples(stepper.server_app.world(), peers, tick)
            && stepper
                .client_apps
                .iter()
                .all(|app| has_position_samples(app.world(), peers, tick))
        {
            return tick;
        }
        tick = tick - 1;
    }
    log_entity_availability("server", stepper.server_app.world_mut(), peers);
    for (i, app) in stepper.client_apps.iter_mut().enumerate() {
        let label = if i == 0 { "client0" } else { "client1" };
        log_entity_availability(label, app.world_mut(), peers);
    }
    panic!("no common deterministic position sample found near {latest_tick:?}");
}

fn log_entity_availability(label: &str, world: &mut World, peers: &[PeerId]) {
    let local_tick = world.resource::<LocalTimeline>().tick();
    let mut latest_by_peer = Vec::new();
    {
        let samples = world.resource::<PositionSamples>();
        for peer in peers {
            let latest = samples
                .0
                .keys()
                .filter_map(|(tick, sample_peer)| (*sample_peer == *peer).then_some(*tick))
                .max();
            latest_by_peer.push((*peer, latest));
        }
    }
    info!(label, ?local_tick, ?latest_by_peer, "sample availability");
    let mut query = world.query::<(
        Entity,
        Option<&DetPlayerId>,
        Option<&DetBallMarker>,
        Option<&Position>,
        Has<DeterministicPredicted>,
        Has<CatchUpGated>,
        Has<DisableRollback>,
    )>();
    let entities: Vec<_> = query
        .iter(world)
        .filter(|(_, player_id, ball, _, _, _, _)| player_id.is_some() || ball.is_some())
        .map(
            |(entity, player_id, ball, position, deterministic, awaiting, rollback_disabled)| {
                (
                    entity,
                    player_id.map(|p| p.0),
                    ball.is_some(),
                    position.copied(),
                    deterministic,
                    awaiting,
                    rollback_disabled,
                )
            },
        )
        .collect();
    info!(label, ?entities, "deterministic entity availability");
}

fn has_position_samples(world: &World, peers: &[PeerId], tick: Tick) -> bool {
    let samples = world.resource::<PositionSamples>();
    peers
        .iter()
        .all(|peer| samples.0.contains_key(&(tick.0, *peer)))
}

fn has_motion_samples(world: &World, subjects: &[MotionSubject], tick: Tick) -> bool {
    let samples = world.resource::<MotionSamples>();
    subjects
        .iter()
        .all(|subject| samples.0.contains_key(&(tick.0, *subject)))
}

fn latest_common_motion_sample_tick(
    stepper: &mut DetStepper,
    subjects: &[MotionSubject],
    latest_tick: Tick,
) -> Tick {
    let mut tick = latest_tick;
    for _ in 0..300 {
        if has_motion_samples(stepper.server_app.world(), subjects, tick)
            && stepper
                .client_apps
                .iter()
                .all(|app| has_motion_samples(app.world(), subjects, tick))
        {
            return tick;
        }
        tick = tick - 1;
    }
    panic!("no common deterministic motion sample found near {latest_tick:?} for {subjects:?}");
}

/// Exercises state-based deterministic catch-up, the bundled forced rollback,
/// and sustained random inputs after catch-up has settled.
#[test]
fn test_state_based_catchup_two_clients() {
    let mut stepper = DetStepper::new_server_with_protocol(DetProtocolPlugin {
        compound_ball: true,
        ..default()
    });
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    // Warmup of 50 ticks keeps inputs zero during catch-up + reconciliation
    // so the catch-up machinery and physics settle before randomness starts.
    configure_stepper(&mut stepper, 50);

    stepper.start();
    stepper.connect_all();

    // Spawn player entities on the server (gated = catch-up required).
    let server_player_a = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(0),
        Vec2::new(-20.0, 0.0),
        true,
    );
    let server_player_b = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(1),
        Vec2::new(20.0, 0.0),
        true,
    );

    // Let structural replication settle so each client has its own
    // `DetPlayerId` + `Player` component.
    stepper.frame_step(15);

    // On each client, find the local player entity and spawn the matching
    // local action entity (`PreSpawned` same hash as on server).
    for client_id in 0..2 {
        let peer = PeerId::Netcode(client_id as u64);
        let server_player = match client_id {
            0 => server_player_a,
            1 => server_player_b,
            _ => unreachable!(),
        };
        let local_player = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player)
            .expect("client should have received its own player by now");
        spawn_local_action_on_client(stepper.client_app(client_id), local_player, peer);
    }

    // Let catch-up + forced rollback + sustained random motion happen.
    stepper.frame_step(200);

    let server_tick = stepper
        .server_app
        .world()
        .resource::<LocalTimeline>()
        .tick();
    let compare_tick = (0..2)
        .map(|client_id| {
            stepper
                .client_app(client_id)
                .world()
                .resource::<LocalTimeline>()
                .tick()
        })
        .fold(server_tick, |min_tick, tick| {
            if tick.0 < min_tick.0 { tick } else { min_tick }
        });
    let peer_a = PeerId::Netcode(0);
    let peer_b = PeerId::Netcode(1);
    let server_pos_a = fixed_position_at(stepper.server_app.world(), peer_a, compare_tick);
    let server_pos_b = fixed_position_at(stepper.server_app.world(), peer_b, compare_tick);

    info!(
        ?server_tick,
        ?compare_tick,
        ?server_pos_a,
        ?server_pos_b,
        "final server fixed positions"
    );

    // Assert that each client's view of both players matches the server.
    for client_id in 0..2 {
        let _client_player_a = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_a)
            .expect("client missing player A entity");
        let _client_player_b = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_b)
            .expect("client missing player B entity");

        let client_tick = stepper
            .client_app(client_id)
            .world()
            .resource::<LocalTimeline>()
            .tick();
        let c_pos_a =
            fixed_position_at(stepper.client_app(client_id).world(), peer_a, compare_tick);
        let c_pos_b =
            fixed_position_at(stepper.client_app(client_id).world(), peer_b, compare_tick);
        info!(
            client_id,
            ?client_tick,
            ?compare_tick,
            ?c_pos_a,
            ?c_pos_b,
            "final client fixed positions"
        );
        assert_relative_eq!(c_pos_a.x, server_pos_a.x, epsilon = 0.01);
        assert_relative_eq!(c_pos_a.y, server_pos_a.y, epsilon = 0.01);
        assert_relative_eq!(c_pos_b.x, server_pos_b.x, epsilon = 0.01);
        assert_relative_eq!(c_pos_b.y, server_pos_b.y, epsilon = 0.01);
    }
}

/// Covers the example flow where an existing client has already moved and
/// collided before a second client late-joins and catches up from state.
#[test]
fn test_state_based_catchup_late_join_after_movement() {
    let mut stepper = DetStepper::new_server();
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper(&mut stepper, 50);

    stepper.start();
    stepper.connect_single(0);

    let peer_a = PeerId::Netcode(0);
    let peer_b = PeerId::Netcode(1);
    let server_player_a =
        spawn_player_on_server(&mut stepper.server_app, peer_a, Vec2::new(-20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 0, server_player_a, peer_a);

    // Client 0 moves and collides with the deterministic ball/walls before
    // client 1 exists.
    stepper.frame_step(140);

    stepper.connect_single(1);
    let server_player_b =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 1, server_player_b, peer_b);

    // Let client 1 receive the state snapshot, activate physics, roll back,
    // and replay from the catch-up tick while both clients keep sending input.
    stepper.frame_step(220);

    assert_clean_stepper_player_entities(&mut stepper, &[peer_a, peer_b]);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);
    compare_players_to_server(
        &mut stepper,
        &[server_player_a, server_player_b],
        &[peer_a, peer_b],
    );
    compare_deterministic_motion_to_server(&mut stepper, &[peer_a, peer_b]);
}

/// Covers a late join while the existing client continuously holds movement
/// input, matching the manual reproduction where the first client keeps
/// pressing a key while the second client applies catch-up.
#[test]
fn test_state_based_catchup_late_join_while_first_client_holds_input() {
    let mut stepper = DetStepper::new_server();
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper_with_fixed_drives(&mut stepper, 50, &[Vec2::X, Vec2::ZERO]);

    stepper.start();
    stepper.connect_single(0);

    let peer_a = PeerId::Netcode(0);
    let peer_b = PeerId::Netcode(1);
    let server_player_a =
        spawn_player_on_server(&mut stepper.server_app, peer_a, Vec2::new(-20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 0, server_player_a, peer_a);

    // Client 0 is holding right before client 1 joins, and keeps holding it
    // through the catch-up snapshot and forced rollback.
    stepper.frame_step(140);

    stepper.connect_single(1);
    let server_player_b =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 1, server_player_b, peer_b);

    stepper.frame_step(220);

    assert_clean_stepper_player_entities(&mut stepper, &[peer_a, peer_b]);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);
    compare_players_to_server(
        &mut stepper,
        &[server_player_a, server_player_b],
        &[peer_a, peer_b],
    );
    compare_deterministic_motion_to_server(&mut stepper, &[peer_a, peer_b]);
}

/// Covers the example flow where an existing client has already moved and
/// collided before a second client late-joins, disconnects, then reconnects
/// with the same peer id and catches up again.
#[test]
fn test_state_based_catchup_late_join_reconnect_after_movement() {
    let mut stepper = DetStepper::new_server();
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper(&mut stepper, 50);

    stepper.start();
    stepper.connect_single(0);

    let peer_a = PeerId::Netcode(0);
    let peer_b = PeerId::Netcode(1);
    let server_player_a =
        spawn_player_on_server(&mut stepper.server_app, peer_a, Vec2::new(-20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 0, server_player_a, peer_a);

    // Client 0 moves and collides before the second client appears.
    stepper.frame_step(140);

    stepper.connect_single(1);
    let server_player_b =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 1, server_player_b, peer_b);

    // Continue with both clients driving random inputs, then disconnect
    // client 1 and remove its server-owned deterministic entities.
    stepper.frame_step(160);
    despawn_server_player_and_action(&mut stepper, server_player_b);
    stepper.disconnect_last_client();
    stepper.frame_step(30);
    assert_clean_stepper_player_entities(&mut stepper, &[peer_a]);
    assert_server_entity_unmapped(&mut stepper, server_player_b);
    assert_clean_stepper_ball_entities(&mut stepper);

    // Recreate client 1 with the same peer id and catch up again while
    // client 0 keeps simulating.
    let reconnected = stepper.new_client();
    assert_eq!(reconnected, 1);
    configure_client_app(stepper.client_app(reconnected), 3, 50);
    stepper.connect_single(reconnected);
    let server_player_b_reconnect =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, reconnected, server_player_b_reconnect, peer_b);

    stepper.frame_step(220);
    assert_clean_stepper_player_entities(&mut stepper, &[peer_a, peer_b]);
    assert_server_entity_unmapped(&mut stepper, server_player_b);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);

    dump_catchup_trace(&mut stepper, &[peer_a, peer_b]);
    compare_players_to_server(
        &mut stepper,
        &[server_player_a, server_player_b_reconnect],
        &[peer_a, peer_b],
    );
    compare_deterministic_motion_to_server(&mut stepper, &[peer_a, peer_b]);
}

/// Covers the deterministic_replication example sequence where every client
/// disconnects, then peer 0 reconnects first and peer 1 late-joins afterward.
#[test]
fn test_state_based_catchup_reconnect_after_all_clients_disconnect() {
    let mut stepper = DetStepper::new_server();
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper(&mut stepper, 50);

    stepper.start();
    stepper.connect_all();

    let peer_a = PeerId::Netcode(0);
    let peer_b = PeerId::Netcode(1);
    let server_player_a =
        spawn_player_on_server(&mut stepper.server_app, peer_a, Vec2::new(-20.0, 0.0), true);
    let server_player_b =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(&mut stepper, 0, server_player_a, peer_a);
    spawn_local_action_after_mapping(&mut stepper, 1, server_player_b, peer_b);

    stepper.frame_step(200);
    assert_clean_stepper_player_entities(&mut stepper, &[peer_a, peer_b]);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);

    despawn_server_player_and_action(&mut stepper, server_player_b);
    despawn_server_player_and_action(&mut stepper, server_player_a);
    stepper.disconnect_last_client();
    stepper.disconnect_last_client();
    stepper.frame_step(30);
    assert_clean_stepper_player_entities(&mut stepper, &[]);
    assert_clean_stepper_ball_entities(&mut stepper);

    let reconnected_a = stepper.new_client();
    assert_eq!(reconnected_a, 0);
    configure_client_app(stepper.client_app(reconnected_a), 3, 50);
    stepper.connect_single(reconnected_a);

    stepper.frame_step(120);
    assert_clean_stepper_player_entities(&mut stepper, &[]);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);

    let server_player_a_reconnect =
        spawn_player_on_server(&mut stepper.server_app, peer_a, Vec2::new(-20.0, 0.0), true);
    spawn_local_action_after_mapping(
        &mut stepper,
        reconnected_a,
        server_player_a_reconnect,
        peer_a,
    );
    stepper.frame_step(120);
    assert_clean_stepper_player_entities(&mut stepper, &[peer_a]);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);

    let reconnected_b = stepper.new_client();
    assert_eq!(reconnected_b, 1);
    configure_client_app(stepper.client_app(reconnected_b), 4, 50);
    stepper.connect_single(reconnected_b);
    let server_player_b_reconnect =
        spawn_player_on_server(&mut stepper.server_app, peer_b, Vec2::new(20.0, 0.0), true);
    spawn_local_action_after_mapping(
        &mut stepper,
        reconnected_b,
        server_player_b_reconnect,
        peer_b,
    );

    stepper.frame_step(220);
    assert_clean_stepper_player_entities(&mut stepper, &[peer_a, peer_b]);
    assert_server_entity_unmapped(&mut stepper, server_player_a);
    assert_server_entity_unmapped(&mut stepper, server_player_b);
    assert_clean_stepper_ball_entities(&mut stepper);
    assert_stepper_catchup_complete(&mut stepper);
}
