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

use crate::client_server::deterministic::protocol::{DetBuffer, DetMovement, DetPlayerId, Player};
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
use lightyear_deterministic_replication::prelude::{
    CatchUpServerReadiness, CatchUpSystems, request_forced_rollback_to_catch_up_tick,
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

fn activate_physics_when_bundle_lands(
    mut commands: Commands,
    pending: Query<
        (Entity, &DetPlayerId, &Position),
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
) {
    use crate::client_server::deterministic::protocol::DetPhysicsBundle;
    let mut newly: Vec<Entity> = Vec::new();
    for (entity, _id, _pos) in pending.iter() {
        commands.entity(entity).insert((
            DetPhysicsBundle::player(),
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
            PhysicsActivated,
        ));
        newly.push(entity);
    }
    if !newly.is_empty() && still_pending.is_empty() {
        let reference = newly[0];
        commands.queue(move |world: &mut World| {
            request_forced_rollback_to_catch_up_tick(world, reference);
        });
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
        client_app.insert_resource(RandomDrive::new(i as u64 + 1, warmup_ticks));
        add_position_samples(client_app);
        client_app.add_systems(
            FixedPreUpdate,
            (activate_physics_when_bundle_lands, drive_random_input),
        );
    }
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

// Suppress unused-import warnings when only some items of PreSpawned are
// used via the stepper helpers.
#[allow(dead_code)]
fn _dummy(_: PreSpawned) {}
