//! Scenario 1: `CatchUpMode::InputOnly` with BEI `Axis2D` inputs.
//!
//! Two clients connect. No state snapshot is ever sent. The server spawns
//! each player entity with `DeterministicPredicted` + physics at connect
//! time; clients receive the initial state via `replicate_once`, activate
//! physics locally, then simulate forward driven purely by replicated
//! inputs.
//!
//! For the first 50 ticks after connect we drive zero-valued inputs so
//! every peer exchanges "all-released" input messages and reaches a
//! steady state. Then both clients start driving random Axis2D inputs
//! every tick. We assert final positions match the server.

use crate::client_server::deterministic::protocol::{
    BALL_SIZE, DetBallMarker, DetBuffer, DetMovement, DetPhysicsBundle, DetPlayerActivationTick,
    DetPlayerId, DetProtocolPlugin, Player,
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
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_messages::MessageManager;
use std::collections::HashMap;
use test_log::test;

#[derive(Resource, Clone)]
struct RandomDrive {
    seed: u64,
    warmup_ticks: u32,
}

#[derive(Resource, Default)]
struct PositionSamples(HashMap<(u32, PeerId), Position>);

impl RandomDrive {
    fn new(seed: u64, warmup_ticks: u32) -> Self {
        Self {
            seed: seed.wrapping_add(0x9E3779B97F4A7C15),
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

fn drive_random_input(
    random: Res<RandomDrive>,
    timeline: Res<LocalTimeline>,
    mut actions: Query<&mut ActionMock, With<Action<DetMovement>>>,
) {
    let tick = timeline.tick();
    let dir = if tick.0 < random.warmup_ticks {
        Vec2::ZERO
    } else {
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

fn configure_stepper(stepper: &mut DetStepper, warmup_ticks: u32) {
    // InputOnly mode on every peer — never send CatchUpRequest, never
    // wait for an accepted catch-up snapshot.
    stepper.server_app.insert_resource(CatchUpMode::InputOnly);
    add_phase_samplers(&mut stepper.server_app, "server");
    for (i, client_app) in stepper.client_apps.iter_mut().enumerate() {
        client_app.insert_resource(CatchUpMode::InputOnly);
        client_app.insert_resource(RandomDrive::new(i as u64 + 1, warmup_ticks));
        client_app.add_systems(FixedPreUpdate, drive_random_input);
        // Mark every replicated `DetPlayerId` entity as
        // `DeterministicPredicted` so that `PredictionHistory<C>` is
        // auto-inserted and input-mismatch rollback replays restore
        // velocity/position before `apply_movement` re-runs. Without
        // this, the rollback replay's second call to `apply_movement`
        // integrates velocity on top of the pre-rollback value, doubling
        // the delta.
        //
        // In InputOnly mode we do NOT insert `AwaitingCatchUpSnapshot`,
        // so `add_confirmed_write`'s history gate doesn't fire — the
        // initial replicated Position lands on the live component.
        client_app.add_systems(
            PreUpdate,
            activate_replicated_players_at_tick.after(ReplicationSystems::Receive),
        );
        let role = if i == 0 { "client_0" } else { "client_1" };
        add_phase_samplers(client_app, role);
    }
}

/// Window of ticks we care about sampling. Sampling every tick floods logs;
/// this range covers the entire random-input phase plus a little lead-in.
const SAMPLE_WINDOW: core::ops::RangeInclusive<u32> = 46..=250;

/// Log Position/LinearVelocity for each player at multiple FixedMain phases
/// on both server and clients, to identify the first drift in tick 61.
fn add_phase_samplers(app: &mut App, role: &'static str) {
    app.init_resource::<PositionSamples>();
    fn sample(
        phase: &'static str,
        role: &'static str,
    ) -> impl FnMut(
        Res<LocalTimeline>,
        ResMut<PositionSamples>,
        Query<
            (
                &DetPlayerId,
                &Position,
                &LinearVelocity,
                &Rotation,
                &AngularVelocity,
            ),
            With<DeterministicPredicted>,
        >,
    ) {
        move |tl: Res<LocalTimeline>,
              mut samples: ResMut<PositionSamples>,
              q: Query<
            (
                &DetPlayerId,
                &Position,
                &LinearVelocity,
                &Rotation,
                &AngularVelocity,
            ),
            With<DeterministicPredicted>,
        >| {
            let tick = tl.tick().0;
            for (id, pos, _, _, _) in q.iter() {
                samples.0.insert((tick, id.0), *pos);
            }
            if !SAMPLE_WINDOW.contains(&tick) {
                return;
            }
            for (id, pos, vel, rot, avel) in q.iter() {
                info!(?role, ?tick, ?phase, player=?id.0, pos=?pos.0, vel=?vel.0, rot_bits=rot.as_radians().to_bits(), avel_bits=avel.0.to_bits(), "sample");
            }
        }
    }
    app.add_systems(FixedLast, sample("FixedLast_end", role));
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

fn latest_real_input_covered_tick(world: &mut World, local_peer_id: PeerId) -> Tick {
    use bevy::ecs::relationship::Relationship;

    let local_tick = world.resource::<LocalTimeline>().tick();
    let mut covered_tick = local_tick;
    let mut players = world.query::<(Entity, &DetPlayerId)>();
    let player_rows = players
        .iter(world)
        .map(|(entity, player_id)| (entity, player_id.0))
        .collect::<Vec<_>>();
    let mut actions = world.query::<(&ActionOf<Player>, &DetBuffer)>();
    for (player, player_id) in player_rows {
        let player_covered_tick = actions.iter(world).find_map(|(action_of, buffer)| {
            (action_of.get() == player).then(|| {
                if player_id == local_peer_id {
                    buffer.end_tick()
                } else {
                    buffer.last_remote_tick
                }
            })
        });
        let Some(Some(player_covered_tick)) = player_covered_tick else {
            return Tick(0);
        };
        if player_covered_tick.0 < covered_tick.0 {
            covered_tick = player_covered_tick;
        }
    }
    covered_tick
}

/// Insert the local physics bundle (Collider/RigidBody) +
/// `DeterministicPredicted` once the replicated player exists locally.
/// Movement is still gated by `DetPlayerActivationTick` in shared logic.
fn activate_replicated_players_at_tick(
    players: Query<
        (Entity, &DetPlayerId),
        (
            With<Position>,
            With<DetPlayerActivationTick>,
            Without<DeterministicPredicted>,
        ),
    >,
    mut commands: Commands,
) {
    let mut ready = players
        .iter()
        .map(|(entity, player_id)| (entity, player_id.0))
        .collect::<Vec<_>>();
    ready.sort_by_key(|(_, player_id)| player_id.to_bits());
    for (entity, _) in ready {
        commands.entity(entity).insert((
            DetPhysicsBundle::player(),
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
        ));
    }
}

const ISLAND_STRESS_BALL_COUNT: usize = 24;

fn add_island_stress_balls(app: &mut App) {
    app.add_systems(Startup, spawn_island_stress_balls);
}

fn install_island_stress_balls(stepper: &mut DetStepper) {
    add_island_stress_balls(&mut stepper.server_app);
    for client_app in &mut stepper.client_apps {
        add_island_stress_balls(client_app);
    }
}

fn spawn_island_stress_balls(mut commands: Commands) {
    let spacing = BALL_SIZE * 1.65;
    for row in 0..4 {
        for col in 0..6 {
            let idx = row * 6 + col;
            let offset = Vec2::new(col as f32 - 2.5, row as f32 - 1.5);
            let position = offset * spacing;
            let inward = -position.normalize_or_zero();
            let swirl = Vec2::new(-offset.y, offset.x).normalize_or_zero();
            let velocity = inward * 45.0 + swirl * 18.0;
            commands.spawn((
                Position::from_xy(position.x, position.y),
                Rotation::default(),
                LinearVelocity(velocity),
                AngularVelocity(0.0),
                DetPhysicsBundle::ball(),
                DetBallMarker,
                DeterministicPredicted {
                    skip_despawn: true,
                    ..default()
                },
                Name::new(format!("IslandStressBall{idx}")),
            ));
        }
    }
}

fn assert_island_stress_balls_are_finite(world: &mut World) {
    let mut balls = world.query_filtered::<&Position, With<DetBallMarker>>();
    let mut count = 0;
    for position in balls.iter(world) {
        assert!(
            position.x.is_finite() && position.y.is_finite(),
            "stress ball position should stay finite: {:?}",
            position.0
        );
        count += 1;
    }
    assert!(
        count >= ISLAND_STRESS_BALL_COUNT + 1,
        "expected default ball plus {ISLAND_STRESS_BALL_COUNT} stress balls, found {count}"
    );
}

/// Exercises random input-only deterministic replication with rollback
/// replays. This catches both input-history issues and physics-side
/// rollback state that is not represented by replicated components.
#[test]
fn test_input_only_two_clients() {
    let mut stepper = DetStepper::new_server();
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper(&mut stepper, 50);

    stepper.start();
    stepper.connect_all();

    // Spawn players on the server WITHOUT catch-up gating — clients receive
    // the initial Position via `replicate_once` at spawn time.
    let server_player_a = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(0),
        Vec2::new(-20.0, 0.0),
        false,
    );
    let server_player_b = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(1),
        Vec2::new(20.0, 0.0),
        false,
    );

    // Let replication settle so clients receive the initial player state.
    stepper.frame_step(15);

    // On each client, spawn the matching local action entity (PreSpawned).
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

    // Warmup (50 ticks of zero input) + random-input exchange phase.
    stepper.frame_step(200);

    let server_tick = stepper
        .server_app
        .world()
        .resource::<LocalTimeline>()
        .tick();
    let compare_tick = (0..2)
        .map(|client_id| {
            latest_real_input_covered_tick(
                stepper.client_app(client_id).world_mut(),
                PeerId::Netcode(client_id as u64),
            )
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

    for client_id in 0..2 {
        let _c_a = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_a)
            .expect("client missing player A");
        let _c_b = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_b)
            .expect("client missing player B");
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

/// Keeps Avian's island graph enabled while rollback replays an input-only
/// physics scene with a dense set of local colliders. This used to panic when
/// the intermediate rollback island state was not restored.
#[test]
fn test_input_only_islands_many_colliders_small_box() {
    let mut stepper = DetStepper::new_server_with_protocol(DetProtocolPlugin {
        enable_islands: true,
    });
    let _c0 = stepper.new_client();
    let _c1 = stepper.new_client();

    configure_stepper(&mut stepper, 50);
    install_island_stress_balls(&mut stepper);

    stepper.start();
    stepper.connect_all();

    let server_player_a = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(0),
        Vec2::new(-20.0, 0.0),
        false,
    );
    let server_player_b = spawn_player_on_server(
        &mut stepper.server_app,
        PeerId::Netcode(1),
        Vec2::new(20.0, 0.0),
        false,
    );

    stepper.frame_step(15);

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

    stepper.frame_step(200);

    assert_island_stress_balls_are_finite(stepper.server_app.world_mut());
    for client_app in &mut stepper.client_apps {
        assert_island_stress_balls_are_finite(client_app.world_mut());
    }
}
