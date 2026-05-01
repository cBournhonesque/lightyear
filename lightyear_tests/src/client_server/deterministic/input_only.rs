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

use crate::client_server::deterministic::protocol::{DetMovement, DetPhysicsBundle, DetPlayerId};
use crate::client_server::deterministic::stepper::{
    DetStepper, spawn_local_action_on_client, spawn_player_on_server,
};
use approx::assert_relative_eq;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Action, ActionMock, ActionValue, MockSpan, TriggerState};
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::CatchUpMode;
use lightyear_messages::MessageManager;
use test_log::test;

#[derive(Resource, Clone)]
struct RandomDrive {
    rng_state: u64,
    ticks: u32,
    warmup_ticks: u32,
}

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

fn drive_random_input(
    mut random: ResMut<RandomDrive>,
    mut actions: Query<&mut ActionMock, With<Action<DetMovement>>>,
) {
    random.ticks += 1;
    let dir = if random.ticks < random.warmup_ticks {
        Vec2::ZERO
    } else {
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

fn configure_stepper(stepper: &mut DetStepper, warmup_ticks: u32) {
    // InputOnly mode on every peer — never send CatchUpRequest, never
    // wait for server readiness.
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
        client_app.add_observer(activate_replicated_player);
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
    fn sample(
        phase: &'static str,
        role: &'static str,
    ) -> impl FnMut(
        Res<LocalTimeline>,
        Query<
            (
                Entity,
                &Position,
                &LinearVelocity,
                &Rotation,
                &AngularVelocity,
            ),
            With<DeterministicPredicted>,
        >,
    ) {
        move |tl: Res<LocalTimeline>,
              q: Query<
            (
                Entity,
                &Position,
                &LinearVelocity,
                &Rotation,
                &AngularVelocity,
            ),
            With<DeterministicPredicted>,
        >| {
            let tick = tl.tick().0;
            if !SAMPLE_WINDOW.contains(&tick) {
                return;
            }
            for (e, pos, vel, rot, avel) in q.iter() {
                info!(?role, ?tick, ?phase, entity=?e, pos=?pos.0, vel=?vel.0, rot_bits=rot.as_radians().to_bits(), avel_bits=avel.0.to_bits(), "sample");
            }
        }
    }
    app.add_systems(FixedLast, sample("FixedLast_end", role));
}

/// On the client, when a replicated `DetPlayerId` entity arrives, insert
/// the local physics bundle (Collider/RigidBody) + `DeterministicPredicted`.
/// Without this the entity has Position/Rotation but nothing for Avian to
/// simulate → player stays stuck at its spawn value.
fn activate_replicated_player(
    trigger: On<Add, DetPlayerId>,
    query: Query<(), (With<Position>, Without<DeterministicPredicted>)>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert((
            DetPhysicsBundle::player(),
            DeterministicPredicted {
                skip_despawn: true,
                ..default()
            },
        ));
    }
}

/// KNOWN-FAILING under random inputs (passes with `warmup_ticks=10000`).
///
/// Root cause (still reproducible after gating `add_confirmed_write` on
/// `AwaitingCatchUpSnapshot`): `Fire<DetMovement>` fires TWICE per tick
/// on the client. Once from the normal `FixedMain` run, then again from
/// the input-mismatch rollback replay. `apply_movement` uses
/// `velocity.x += delta`, which is non-idempotent — the second call
/// doubles the delta.
///
/// Fix options:
/// - Make `apply_movement` write absolute velocity (e.g. `velocity.xy =
///   input * speed`) instead of accumulating deltas. Idempotent under
///   repeated firing. This is probably the correct user-code pattern for
///   BEI + deterministic replication.
/// - Or: have BEI's `Apply` system skip re-firing during rollback replay
///   by default, requiring user code to opt in to re-fire if needed.
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

    let server_pos_a = *stepper
        .server_app
        .world()
        .get::<Position>(server_player_a)
        .expect("server player A missing Position");
    let server_pos_b = *stepper
        .server_app
        .world()
        .get::<Position>(server_player_b)
        .expect("server player B missing Position");
    info!(?server_pos_a, ?server_pos_b, "final server positions");

    for client_id in 0..2 {
        let c_a = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_a)
            .expect("client missing player A");
        let c_b = stepper
            .client(client_id)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_player_b)
            .expect("client missing player B");
        let c_pos_a = *stepper
            .client_app(client_id)
            .world()
            .get::<Position>(c_a)
            .expect("client player A missing Position");
        let c_pos_b = *stepper
            .client_app(client_id)
            .world()
            .get::<Position>(c_b)
            .expect("client player B missing Position");
        assert_relative_eq!(c_pos_a.x, server_pos_a.x, epsilon = 0.01);
        assert_relative_eq!(c_pos_a.y, server_pos_a.y, epsilon = 0.01);
        assert_relative_eq!(c_pos_b.x, server_pos_b.x, epsilon = 0.01);
        assert_relative_eq!(c_pos_b.y, server_pos_b.y, epsilon = 0.01);
    }
}
