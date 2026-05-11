//! Scenario 2: `CatchUpMode::StateBasedCatchUp` with BEI `Axis2D` inputs,
//! random per-tick movements and tight collisions.
//!
//! Two clients connect. The server delays the bundled catch-up snapshot
//! (via `CatchUpServerReadiness`) until it has received inputs from every
//! connected client. Each client receives the bundle in one replicon
//! update and fires a single forced rollback to reconcile.
//!
//! Both clients then drive their player with randomised Axis2D inputs
//! switching every tick, inside a small box that forces frequent
//! player↔player and player↔ball collisions. We assert that the final
//! Position on each client matches the server bit-perfectly (via Avian's
//! `enhanced-determinism` feature + our catch-up machinery).

use crate::client_server::deterministic::protocol::{
    DetBallMarker, DetBuffer, DetMovement, DetPlayerId, Player,
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
use lightyear_deterministic_replication::prelude::{
    AwaitingCatchUpSnapshot, CatchUpRequestSent, CatchUpServerReadiness, CatchUpSystems,
    request_forced_rollback_to_catch_up_tick,
};
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::PreSpawned;
use std::collections::HashMap;
use test_log::test;

/// Resource on each client driving a deterministic pseudo-random walk
/// for its BEI action.
#[derive(Resource, Clone)]
struct RandomDrive {
    rng_state: u64,
    /// Number of ticks we've driven so far.
    ticks: u32,
    /// Keep the action still for this many ticks at the start so catch-up
    /// machinery can settle (matches "50 ticks then start moving" request).
    warmup_ticks: u32,
}

#[derive(Resource, Default)]
struct PositionSamples(HashMap<(u32, PeerId), Position>);

impl RandomDrive {
    fn new(seed: u64, warmup_ticks: u32) -> Self {
        Self {
            rng_state: seed.wrapping_add(0x9E3779B97F4A7C15),
            ticks: 0,
            warmup_ticks,
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
    mut random: ResMut<RandomDrive>,
    mut actions: Query<&mut ActionMock, With<Action<DetMovement>>>,
) {
    random.ticks += 1;
    let dir = if random.ticks < random.warmup_ticks {
        Vec2::ZERO
    } else {
        // Pick one of 4 cardinal directions pseudo-randomly.
        let r = splitmix64(&mut random.rng_state);
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

/// Server-side: update `CatchUpServerReadiness` based on whether every
/// player has real remote input through the server's current tick.
fn update_server_readiness(
    timeline: Res<LocalTimeline>,
    players: Query<Entity, With<DetPlayerId>>,
    actions: Query<(&ActionOf<Player>, &DetBuffer)>,
    mut readiness: ResMut<CatchUpServerReadiness>,
) {
    use bevy::ecs::relationship::Relationship;
    let current_tick = timeline.tick();
    let mut any = false;
    let ready = players.iter().all(|player_entity| {
        any = true;
        actions.iter().any(|(action_of, buffer)| {
            action_of.get() == player_entity
                && matches!(buffer.last_remote_tick, Some(t) if t >= current_tick)
        })
    });
    readiness.all_clients_ready = any && ready;
}

/// Client-side: once the catch-up bundle for every known player entity
/// has landed (Position present), insert the physics bundle +
/// `DeterministicPredicted` and fire the bundled forced rollback.
#[derive(Component)]
struct PhysicsActivated;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayCoverage {
    Ready,
    Wait,
    Stale,
}

fn mark_awaiting_on_hidden_replicated_players(
    players: Query<
        Entity,
        (
            With<DetPlayerId>,
            Without<Position>,
            Without<AwaitingCatchUpSnapshot>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &players {
        commands.entity(entity).insert(AwaitingCatchUpSnapshot);
    }
}

fn activate_physics_when_bundle_lands(
    mut commands: Commands,
    pending: Query<
        (
            Entity,
            &DetPlayerId,
            &Position,
            Has<AwaitingCatchUpSnapshot>,
        ),
        (With<DetPlayerId>, Without<PhysicsActivated>),
    >,
    still_pending: Query<
        Entity,
        (
            With<DetPlayerId>,
            Without<PhysicsActivated>,
            Without<Position>,
        ),
    >,
    awaiting_snapshots: Query<(Entity, Option<&ConfirmHistory>), With<AwaitingCatchUpSnapshot>>,
    checkpoints: Res<ReplicationCheckpointMap>,
    timeline: Res<LocalTimeline>,
    local_id: Option<Single<&LocalId, With<Client>>>,
    client_request: Option<Single<Entity, (With<Client>, With<CatchUpRequestSent>)>>,
    prediction_manager: Option<Single<&PredictionManager, With<Client>>>,
    all_players: Query<(Entity, &DetPlayerId)>,
    mut input_buffers: Query<(&ActionOf<Player>, Option<&mut DetBuffer>)>,
) {
    use crate::client_server::deterministic::protocol::DetPhysicsBundle;
    let reference = catchup_snapshot_reference(&awaiting_snapshots, &checkpoints);
    let max_rollback_ticks = prediction_manager
        .as_ref()
        .map(|manager| manager.rollback_policy.max_rollback_ticks)
        .unwrap_or(100);
    let coverage = if still_pending.is_empty() {
        reference.map_or(ReplayCoverage::Wait, |reference| {
            let Some(local_id) = local_id.as_ref() else {
                return ReplayCoverage::Wait;
            };
            let reference_tick = checkpoints
                .get(
                    awaiting_snapshots
                        .get(reference)
                        .ok()
                        .and_then(|(_, confirm)| confirm.map(ConfirmHistory::last_tick))
                        .expect("catch-up reference has a confirmation tick"),
                )
                .expect("catch-up reference has a replication checkpoint");
            input_buffers_cover_replay(
                reference_tick,
                timeline.tick(),
                local_id.0,
                max_rollback_ticks,
                &all_players,
                &mut input_buffers,
            )
        })
    } else {
        ReplayCoverage::Wait
    };
    if coverage == ReplayCoverage::Stale
        && let Some(client_request) = client_request
    {
        commands
            .entity(client_request.into_inner())
            .remove::<CatchUpRequestSent>();
    }
    let catchup_ready = coverage == ReplayCoverage::Ready;

    let mut newly: Vec<Entity> = Vec::new();
    let mut activated_awaiting_catchup = false;
    for (entity, _id, _pos, awaiting_catchup) in pending.iter() {
        if awaiting_catchup && !catchup_ready {
            continue;
        }
        commands.entity(entity).insert((
            DetPhysicsBundle::player(),
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
            PhysicsActivated,
        ));
        newly.push(entity);
        activated_awaiting_catchup |= awaiting_catchup;
    }
    if catchup_ready
        && (activated_awaiting_catchup || (!awaiting_snapshots.is_empty() && newly.is_empty()))
        && let Some(reference) = reference
    {
        commands.queue(move |world: &mut World| {
            request_forced_rollback_to_catch_up_tick(world, reference);
        });
    }
}

fn catchup_snapshot_reference(
    awaiting_snapshots: &Query<(Entity, Option<&ConfirmHistory>), With<AwaitingCatchUpSnapshot>>,
    checkpoints: &ReplicationCheckpointMap,
) -> Option<Entity> {
    let mut reference = None;
    let mut bundled_tick = None;
    for (entity, confirm) in awaiting_snapshots.iter() {
        let confirm = confirm?;
        let tick = confirm.last_tick();
        checkpoints.get(tick)?;
        match bundled_tick {
            Some(expected) if expected != tick => return None,
            Some(_) => {}
            None => {
                bundled_tick = Some(tick);
                reference = Some(entity);
            }
        }
    }
    reference
}

fn input_buffers_cover_replay(
    reference_tick: Tick,
    local_tick: Tick,
    local_id: PeerId,
    max_rollback_ticks: u16,
    players: &Query<(Entity, &DetPlayerId)>,
    input_buffers: &mut Query<(&ActionOf<Player>, Option<&mut DetBuffer>)>,
) -> ReplayCoverage {
    use bevy::ecs::relationship::Relationship;

    if local_tick - reference_tick > i32::from(max_rollback_ticks) {
        return ReplayCoverage::Stale;
    }
    let mut any_player = false;
    for (player, player_id) in players.iter() {
        any_player = true;
        let mut found = false;
        for (action_of, buffer) in input_buffers.iter_mut() {
            if action_of.get() != player {
                continue;
            }
            let Some(mut buffer) = buffer else {
                continue;
            };
            if player_id.0 == local_id {
                let replay_end_tick = local_tick - 1;
                let Some(end_tick) = buffer.end_tick() else {
                    return ReplayCoverage::Wait;
                };
                if end_tick < replay_end_tick {
                    return ReplayCoverage::Wait;
                }
                if buffer.start_tick.is_none_or(|start| start > reference_tick) {
                    let old_start = buffer.start_tick.unwrap_or(reference_tick);
                    buffer.extend_to_range(reference_tick, end_tick);
                    let mut tick = reference_tick;
                    while tick < old_start {
                        buffer.set(
                            tick,
                            ActionsSnapshot {
                                state: TriggerState::None,
                                value: ActionValue::Axis2D(Vec2::ZERO),
                                ..default()
                            },
                        );
                        tick = tick + 1;
                    }
                }
            } else if buffer.start_tick.is_none_or(|start| start > reference_tick) {
                return ReplayCoverage::Stale;
            } else if buffer
                .last_remote_tick
                .is_none_or(|last_remote_tick| last_remote_tick < reference_tick)
            {
                return ReplayCoverage::Wait;
            }
            found = true;
            break;
        }
        if !found {
            return ReplayCoverage::Wait;
        }
    }
    if any_player {
        ReplayCoverage::Ready
    } else {
        ReplayCoverage::Wait
    }
}

/// Wire up the readiness system on the server and the physics-activation
/// + random-drive systems on each client.
fn configure_stepper(stepper: &mut DetStepper, warmup_ticks: u32) {
    stepper.server_app.add_systems(
        PreUpdate,
        update_server_readiness.in_set(CatchUpSystems::UpdateReadiness),
    );
    add_position_samples(&mut stepper.server_app);
    for (i, client_app) in stepper.client_apps.iter_mut().enumerate() {
        configure_client_app(client_app, i as u64 + 1, warmup_ticks);
    }
}

fn configure_client_app(client_app: &mut App, seed: u64, warmup_ticks: u32) {
    client_app.insert_resource(RandomDrive::new(seed, warmup_ticks));
    add_position_samples(client_app);
    client_app.add_systems(
        PreUpdate,
        (
            mark_awaiting_on_hidden_replicated_players
                .after(ReplicationSystems::Receive)
                .before(CatchUpSystems::DetectSnapshotReady),
            activate_physics_when_bundle_lands.in_set(CatchUpSystems::OnSnapshotReady),
        ),
    );
    client_app.add_systems(FixedPreUpdate, drive_random_input);
}

fn add_position_samples(app: &mut App) {
    app.init_resource::<PositionSamples>();
    app.add_systems(FixedLast, sample_positions);
}

fn sample_positions(
    timeline: Res<LocalTimeline>,
    mut samples: ResMut<PositionSamples>,
    players: Query<(&DetPlayerId, &Position), With<DeterministicPredicted>>,
) {
    let tick = timeline.tick().0;
    for (id, position) in &players {
        samples.0.insert((tick, id.0), *position);
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
        Has<PhysicsActivated>,
        Has<AwaitingCatchUpSnapshot>,
        Has<DisableRollback>,
    )>();
    let entities: Vec<_> = query
        .iter(world)
        .filter(|(_, player_id, ball, _, _, _, _, _)| player_id.is_some() || ball.is_some())
        .map(
            |(
                entity,
                player_id,
                ball,
                position,
                deterministic,
                activated,
                awaiting,
                rollback_disabled,
            )| {
                (
                    entity,
                    player_id.map(|p| p.0),
                    ball.is_some(),
                    position.copied(),
                    deterministic,
                    activated,
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

/// Exercises state-based deterministic catch-up, the bundled forced rollback,
/// and sustained random inputs after catch-up has settled.
#[test]
fn test_state_based_catchup_two_clients() {
    let mut stepper = DetStepper::new_server();
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

    compare_players_to_server(
        &mut stepper,
        &[server_player_a, server_player_b_reconnect],
        &[peer_a, peer_b],
    );
}

// Suppress unused-import warnings when only some items of PreSpawned are
// used via the stepper helpers.
#[allow(dead_code)]
fn _dummy(_: PreSpawned) {}
