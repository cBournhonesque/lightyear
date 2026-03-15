use crate::client_server::prediction::{
    register_rollback_check_helper, trigger_rollback_check, trigger_state_rollback,
};
use crate::protocol::{CompFull, CompNotNetworked, NativeInput};
use crate::stepper::*;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::input::native::prelude::InputMarker;
use lightyear::prediction::Predicted;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prelude::input::native::ActionState;
use lightyear_connection::prelude::NetworkTarget;
use lightyear_core::id::PeerId;
use lightyear_core::prelude::LocalTimeline;
use lightyear_messages::MessageManager;
use lightyear_prediction::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
use lightyear_prediction::manager::{LastConfirmedInput, RollbackMode};
use lightyear_prediction::prelude::*;
use lightyear_prediction::rollback::{DeterministicPredicted, reset_input_rollback_tracker};
use lightyear_replication::prelude::*;
use test_log::test;

fn setup() -> (ClientServerStepper, Entity) {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    register_rollback_check_helper(stepper.client_app());

    // add predicted/confirmed entities
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            CompFull(1.0),
        ))
        .id();
    // run one frame to initialize prediction history for the entity
    stepper.frame_step(1);
    (stepper, predicted)
}

// =============================================================================
// Scenario 1: Component predicted-inserted but not from replication → removed on rollback
// =============================================================================

/// Client prediction adds a component, but server never has it.
/// On rollback, the component should be removed.
#[test]
fn test_predicted_insert_reverted_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Client prediction adds CompNotNetworked (not replicated, server doesn't have it)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(1.0));

    // Simulate confirmed state at rollback_tick: CompFull still 1.0 (no change from server)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(rollback_tick, Some(CompFull(1.0)));

    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // CompNotNetworked should be removed: it wasn't in the history at rollback_tick
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompNotNetworked>(predicted)
            .is_none(),
        "Predicted-inserted component should be removed on rollback"
    );
}

// =============================================================================
// Scenario 2: Component predicted-removed → re-inserted on rollback
// =============================================================================

/// Client prediction removes a component, but server still has it.
/// On rollback, the component should be restored.
#[test]
fn test_predicted_remove_restored_on_rollback() {
    let (mut stepper, predicted) = setup();

    fn increment_and_remove(
        mut commands: Commands,
        mut query: Query<(Entity, &mut CompFull), With<Predicted>>,
    ) {
        for (entity, mut comp) in query.iter_mut() {
            comp.0 += 1.0;
            if comp.0 == 5.0 {
                commands.entity(entity).remove::<CompFull>();
            }
        }
    }
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_and_remove);

    // Run until CompFull is removed (1.0 → 2.0 → 3.0 → 4.0 → 5.0 → removed)
    stepper.frame_step(5);
    assert!(
        stepper.client_app().world().get::<CompFull>(predicted).is_none(),
        "CompFull should have been removed by prediction"
    );

    let tick = stepper.client_tick(0);
    // Simulate server confirms CompFull = -10.0 at tick-3 (server says it still exists)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompFull(-10.0));
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(tick - 3, Some(CompFull(-10.0)));

    trigger_rollback_check(&mut stepper, tick - 3);
    stepper.frame_step(1);

    // CompFull should be re-added and re-simulated: -10 + 4 increments = -6
    assert_eq!(
        stepper.client_app().world().get::<CompFull>(predicted).unwrap().0,
        -6.0,
        "Predicted-removed component should be restored and re-simulated on rollback"
    );
}

// =============================================================================
// Scenario 3: Entity predicted-despawned → re-enabled on rollback
// =============================================================================

/// Client uses `prediction_despawn` on an entity.
/// On rollback (to before the despawn), the entity should be re-enabled.
#[test]
fn test_predicted_despawn_restored_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Predicted-despawn the entity (adds PredictionDisable, doesn't actually despawn)
    stepper
        .client_app()
        .world_mut()
        .commands()
        .entity(predicted)
        .prediction_despawn();
    stepper.frame_step(1);

    assert!(
        stepper.client_app().world().get_entity(predicted).is_ok(),
        "Entity should still exist after prediction_despawn"
    );
    assert!(
        stepper.client_app().world().get::<PredictionDisable>(predicted).is_some(),
        "Entity should have PredictionDisable marker"
    );

    // Trigger rollback to before the despawn
    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // PredictionDisable should be removed, entity re-enabled
    assert!(
        stepper.client_app().world().get_entity(predicted).is_ok(),
        "Entity should still exist after rollback"
    );
    assert!(
        stepper.client_app().world().get::<PredictionDisable>(predicted).is_none(),
        "PredictionDisable should be removed after rollback"
    );
    assert_eq!(
        stepper.client_app().world().get::<CompFull>(predicted).unwrap(),
        &CompFull(1.0),
        "Component should be restored to value at rollback tick"
    );
}

// =============================================================================
// Scenario 4: Entity predicted-spawned → despawned on rollback
// =============================================================================

/// A DeterministicPredicted entity spawned during prediction should be despawned
/// if rollback goes back to before the spawn tick.
#[test]
fn test_predicted_spawn_despawned_on_rollback() {
    let (mut stepper, _) = setup();
    stepper.frame_step(1);

    let tick = stepper.client_tick(0);
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, DeterministicPredicted::default(), CompFull(1.0)))
        .id();

    // trigger a rollback to before the entity was spawned
    trigger_state_rollback(&mut stepper, tick - 1);
    stepper.frame_step(1);

    assert!(
        stepper.client_app().world().get_entity(predicted_a).is_err(),
        "Predicted-spawned entity should be despawned on rollback to before spawn tick"
    );
}

// =============================================================================
// Scenario 5: Component modified → corrected on rollback
// =============================================================================

/// Client prediction modifies a component value differently than the server.
/// On rollback, the component should snap to the confirmed value and re-simulate.
#[test]
fn test_predicted_modify_corrected_on_rollback() {
    let (mut stepper, predicted) = setup();

    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);

    // Run 3 frames: CompFull goes 1.0 → 2.0 → 3.0 → 4.0
    stepper.frame_step(3);
    assert_eq!(
        stepper.client_app().world().get::<CompFull>(predicted).unwrap().0,
        4.0
    );

    let tick = stepper.client_tick(0);
    // Server says CompFull was actually 10.0 at tick-2 (different from predicted 2.0)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(tick - 2, Some(CompFull(10.0)));

    trigger_rollback_check(&mut stepper, tick - 2);
    stepper.frame_step(1);

    // Snap to 10.0 at tick-2, re-simulate 3 ticks: 10.0 + 3 = 13.0
    assert_eq!(
        stepper.client_app().world().get::<CompFull>(predicted).unwrap().0,
        13.0,
        "Modified component should be corrected to confirmed value and re-simulated"
    );
}

// =============================================================================
// Scenario 6: Component inserted from remote → applied on rollback
// =============================================================================

/// Server sends a new component that the client didn't have.
/// On rollback, the component should be present at the confirmed tick.
#[test]
fn test_remote_insert_applied_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(2);
    let tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Simulate server sending CompNotNetworked(5.0) at `tick` (component client didn't predict)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(5.0));
    // Create prediction history for CompNotNetworked with confirmed value
    let mut history = PredictionHistory::<CompNotNetworked>::default();
    history.add_confirmed(tick, Some(CompNotNetworked(5.0)));
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(history);

    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);

    // The remotely-inserted component should be present after rollback
    assert_eq!(
        stepper.client_app().world().get::<CompNotNetworked>(predicted).unwrap(),
        &CompNotNetworked(5.0),
        "Remotely-inserted component should be present after rollback"
    );
}

// =============================================================================
// Scenario 7: Component removed from remote → removed on rollback
// =============================================================================

/// Server removes a component that the client still had.
/// On rollback, the component should be absent.
#[test]
fn test_remote_remove_applied_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Simulate server removing CompFull at rollback_tick
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(rollback_tick, None);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<CompFull>();

    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // CompFull should be absent after rollback: server confirmed removal
    assert!(
        stepper.client_app().world().get::<CompFull>(predicted).is_none(),
        "Remotely-removed component should be absent after rollback"
    );
}

// =============================================================================
// Other rollback tests
// =============================================================================

/// If we have disable_rollback (DeterministicPredicted):
/// 1) the entity alone doesn't trigger rollback
/// 2) if a rollback happens (from another entity), we reset to the predicted history value
#[test]
fn test_disable_rollback() {
    let (mut stepper, predicted_b) = setup();

    // add a DeterministicPredicted entity (disable state rollback for it)
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            DeterministicPredicted::default(),
            CompFull(1.0),
        ))
        .id();

    // value gets synced and added to PredictionHistory
    stepper.frame_step(1);

    // 2. If a rollback happens (triggered by predicted_b), DeterministicPredicted entity
    //    gets reset to its historical value
    let tick = stepper.client_tick(0);

    // Set up history for predicted_a with a known confirmed value
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_a)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(tick, Some(CompFull(10.0)));

    // Simulate confirmed update for predicted_b with a different value to trigger mismatch
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_b)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_confirmed(tick, Some(CompFull(3.0)));
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_b)
        .get_mut::<CompFull>()
        .unwrap()
        .0 = 3.0;

    // step once to avoid a 0-tick rollback
    stepper.frame_step(1);

    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);

    // the DeterministicPredicted entity was rolled back to the past PredictionHistory value
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_a)
            .unwrap()
            .0,
        10.0
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_b)
            .unwrap()
            .0,
        3.0
    );
}

/// Test that:
/// - the `Time` resource's elapsed is rollbacked to the first tick of the rollback
/// - the `Time` resource's elapsed time is advanced correctly during the rollback
/// - the `Time` resource's delta during a rollback is the `Time<Fixed>`'s delta
#[test]
fn test_rollback_time_resource() {
    #[derive(Debug, PartialEq)]
    struct TimeSnapshot {
        is_rollback: bool,
        delta: Duration,
        elapsed: Duration,
    }

    #[derive(Resource, Default, Debug)]
    struct TimeTracker {
        snapshots: Vec<TimeSnapshot>,
    }

    // Record the time resource's values for each tick.
    fn track_time(
        time: Res<Time>,
        mut time_tracker: ResMut<TimeTracker>,
        rollback: Single<&PredictionManager>,
    ) {
        time_tracker.snapshots.push(TimeSnapshot {
            is_rollback: rollback.is_rollback(),
            delta: time.delta(),
            elapsed: time.elapsed(),
        });
    }

    let (mut stepper, predicted) = setup();
    // Build up enough prediction history so rollback tick is within range
    stepper.frame_step(2);

    // Add time tracking AFTER building history to only capture the rollback frame
    stepper.client_app().insert_resource(TimeTracker::default());
    stepper.client_app().add_systems(FixedUpdate, track_time);
    let time_before_next_tick = *stepper.client_app().world().resource::<Time<Fixed>>();

    // Trigger 2 rollback ticks
    let tick = stepper.client_tick(0);
    trigger_rollback_check(&mut stepper, tick - 2);
    stepper.frame_step(1);

    // Check that the component got synced.
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap(),
        &CompFull(1.0)
    );

    // Verify that the 2 rollback ticks and regular tick occurred with the
    // correct delta times and elapsed times.
    let tick_duration = stepper.tick_duration;
    let time_tracker = stepper.client_app().world().resource::<TimeTracker>();
    assert_eq!(
        time_tracker.snapshots,
        vec![
            TimeSnapshot {
                is_rollback: true,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed() - tick_duration
            },
            TimeSnapshot {
                is_rollback: true,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed()
            },
            TimeSnapshot {
                is_rollback: false,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed() + tick_duration
            }
        ]
    );
}

/// Clients 1 and 2 have inputs and send them to the Server, who rebroadcasts to client 0
fn setup_stepper_for_input_rollback(
    mode: RollbackMode,
) -> (ClientServerStepper, Entity, Entity, Entity, Entity) {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(3));

    let mut client_mut = stepper.client_mut(0);
    let mut prediction_manager = client_mut.get_mut::<PredictionManager>().unwrap();
    prediction_manager.rollback_policy.input = mode;
    prediction_manager.rollback_policy.state = RollbackMode::Disabled;

    let server_entity_1 = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            PeerId::Netcode(2),
        )))
        .id();
    let server_entity_2 = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            PeerId::Netcode(1),
        )))
        .id();
    stepper.frame_step_server_first(1);

    // Check that in PostUpdate, the LastConfirmedInput is reset if no input messages were received
    let client = stepper.client(0);
    assert!(!client.get::<LastConfirmedInput>().unwrap().received_input());

    // add input-markers on client 1/2 so that they can send remote input messages
    let client_entity_1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .expect("entity was not replicated to client");
    stepper.client_apps[1]
        .world_mut()
        .entity_mut(client_entity_1)
        .insert((InputMarker::<NativeInput>::default(),));

    let client_entity_2 = stepper
        .client(2)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");
    stepper.client_apps[2]
        .world_mut()
        .entity_mut(client_entity_2)
        .insert((InputMarker::<NativeInput>::default(),));

    let client_entity_a = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .expect("entity was not replicated to client");
    // we want to predict this entity
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity_a)
        .insert((CompNotNetworked(1.0), DeterministicPredicted::default()));
    let client_entity_b = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    (
        stepper,
        client_entity_1,
        client_entity_2,
        client_entity_a,
        client_entity_b,
    )
}

/// Test that we rollback from the last confirmed input when RollbackMode::Always for inputs
#[test]
fn test_input_rollback_always_mode() {
    let (mut stepper, _, _, client_entity, _) =
        setup_stepper_for_input_rollback(RollbackMode::Always);

    // build a steady state where have already received an input
    stepper.frame_step(2);

    // send input message from client 1/2 to server
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    info!("Will check rollback at tick: {input_tick:?}");

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>,
              manager: Single<(&LastConfirmedInput, &PredictionManager)>| {
            let (last_confirmed_input, manager) = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(last_confirmed_input.received_input());
                let rollback_start = manager.get_rollback_start_tick();
                // We receive the input message for tick `input_tick`, but we rollback at the previous LastConfirmedInput tick,
                // which is `input_tick - 1`
                assert_eq!(rollback_start.unwrap(), input_tick - 1);
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(reset_input_rollback_tracker),
    );

    // modify the CompNotNetworked component
    stepper.client_apps[0]
        .world_mut()
        .get_mut::<CompNotNetworked>(client_entity)
        .unwrap()
        .0 = 2.0;

    // server broadcast input message to clients (including client 0)
    stepper.frame_step_server_first(1);

    // after the rollback, the last_confirmed_input is reset
    assert_eq!(
        stepper
            .client(0)
            .get::<LastConfirmedInput>()
            .unwrap()
            .tick
            .get(),
        input_tick
    );
    // also check that the component was reset to the value it had in the history
    assert_eq!(
        stepper.client_apps[0]
            .world()
            .get::<CompNotNetworked>(client_entity)
            .unwrap()
            .0,
        1.0
    );
}

/// Test that LastConfirmedInput computes the earliest input across multiple clients
#[test]
fn test_last_confirmed_input_multiple_clients() {
    let (mut stepper, client_entity_1, _, _, _) =
        setup_stepper_for_input_rollback(RollbackMode::Always);

    // only client 2 will send an input message to the server
    stepper.client_apps[1]
        .world_mut()
        .entity_mut(client_entity_1)
        .remove::<InputMarker<NativeInput>>();
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);

    // after the rollback, the last_confirmed_input is updated. It's updated to `input_tick - 1` and not `input_tick`
    // because we didn't receive a new input message from client 1
    assert_eq!(
        stepper
            .client(0)
            .get::<LastConfirmedInput>()
            .unwrap()
            .tick
            .get(),
        input_tick - 1
    );
}

/// Test that rollback tick is set to the earliest mismatch when RollbackMode::Check for inputs
#[test]
fn test_input_rollback_check_mode_earliest_mismatch() {
    let (mut stepper, client_entity_1, _, client_entity_a, _) =
        setup_stepper_for_input_rollback(RollbackMode::Check);

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    // client 1 and client 2 send an input message to the server
    // client 1's input will cause a mismatch
    stepper.client_apps[1]
        .world_mut()
        .get_mut::<ActionState<NativeInput>>(client_entity_1)
        .unwrap()
        .0 = NativeInput(1);
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>, manager: Single<&PredictionManager>| {
            let manager = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(manager.earliest_mismatch_input.has_mismatches());
                let rollback_start = manager.get_rollback_start_tick();
                // there is a mismatch only for client 1, which is enough to trigger a rollback.
                // we trigger a rollback to the earliest mismatch, which is `input_tick`
                assert_eq!(rollback_start.unwrap(), input_tick - 1);
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(reset_input_rollback_tracker),
    );

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);
}

/// Test that we don't rollback if there are no input mismatches in Check mode
#[test]
fn test_no_rollback_without_input_mismatches() {
    let (mut stepper, _, _, _, _) = setup_stepper_for_input_rollback(RollbackMode::Check);

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    // client 1 and client 2 send an input message to the server
    // there will be no mismatches
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>, manager: Single<&PredictionManager>| {
            let manager = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(!manager.earliest_mismatch_input.has_mismatches());
                let rollback_start = manager.get_rollback_start_tick();
                assert!(rollback_start.is_none());
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);
}

/// Test that if we spawn a DeterministicPredicted entity with skip_despawn = true
/// We only start enabling rollback for this entity a few ticks after it was spawned.
#[test]
fn test_deterministic_predicted_skip_despawn() {
    let (mut stepper, _) = setup();

    // add predicted/confirmed entities
    let receiver = stepper.client(0).id();
    let tick = stepper.client_tick(0);
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 2,
            },
            CompFull(1.0),
        ))
        .id();

    // Rollback check: the entity should have DisableRollback added
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DisableRollback>(predicted_a)
            .is_some()
    );

    // trigger a rollback at tick + 2, we should re-enable rollback
    // since it's the spawn_tick of DeterministicPredicted + 2
    trigger_state_rollback(&mut stepper, tick + 2);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DisableRollback>(predicted_a)
            .is_none()
    );
}

/// Test that if we spawn a DeterministicPredicted entity with skip_despawn = false
/// The entity is despawned it was spawned before the rollback tick.
#[test]
fn test_deterministic_predicted_despawn() {
    let (mut stepper, _) = setup();
    stepper.frame_step(1);

    // add predicted/confirmed entities
    let receiver = stepper.client(0).id();
    let tick = stepper.client_tick(0);

    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, DeterministicPredicted::default(), CompFull(1.0)))
        .id();

    // trigger a rollback at tick - 2, we should despawn the DeterministicPredicted
    // since it was spawned before the rollback
    trigger_state_rollback(&mut stepper, tick - 1);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(predicted_a)
            .is_err()
    )
}
