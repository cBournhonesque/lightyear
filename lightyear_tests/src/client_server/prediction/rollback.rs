use crate::client_server::prediction::{
    RollbackInfo, trigger_rollback_check, trigger_rollback_system,
};
use crate::protocol::{CompFull, CompNotNetworked, NativeInput};
use crate::stepper::ClientServerStepper;
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
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_messages::MessageManager;
use lightyear_prediction::manager::{LastConfirmedInput, RollbackMode};
use lightyear_prediction::prelude::{PredictionManager, PredictionMetrics, RollbackSet};
use lightyear_prediction::rollback::{DeterministicPredicted, reset_input_rollback_tracker};
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::*;
use test_log::test;

fn setup() -> (ClientServerStepper, Entity) {
    let mut stepper = ClientServerStepper::single();
    stepper.client_app().add_message::<RollbackInfo>();
    stepper.client_app().add_systems(
        PreUpdate,
        trigger_rollback_system
            .after(ReplicationSet::Receive)
            .before(RollbackSet::Check),
    );

    // add predicted/confirmed entities
    let tick = stepper.client_tick(0);
    let receiver = stepper.client(0).id();
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            Replicated { receiver },
            ConfirmedTick { tick },
            Confirmed(CompFull(1.0)),
        ))
        .id();
    // add a rollback check by setting receiver.has_received_this_frame
    trigger_rollback_check(&mut stepper, tick);
    (stepper, predicted)
}

struct RollbackCounter(pub usize);

// TODO: check that if A is updated but B is not, and A and B are in the same replication group,
//  then we need to check the rollback for B as well!
/// Check that we enter a rollback state when confirmed entity is updated at tick T and:
/// 1. Predicted component and Confirmed component are different
/// 2. Confirmed component does not exist and predicted component exists
/// 3. Confirmed component exists but predicted component does not exist
/// 4. If confirmed component is the same value as what we have in the history for predicted component, we do not rollback
#[test]
fn test_check_rollback() {
    let (mut stepper, predicted) = setup();

    // make sure we simulate that we received a server update
    let tick = stepper.client_tick(0);

    // step once to avoid 0 tick rollback
    stepper.frame_step(1);

    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(predicted)
            .is_some()
    );
    let history_id = stepper
        .client_app()
        .world_mut()
        .register_component::<PredictionHistory<CompFull>>();
    info!(?history_id, "hi");
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    // 0. Rollback when the Confirmed component is just added
    // (there is a rollback even though the values match, because the value isn't present in
    //  the PredictionHistory at the time of spawn)
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(predicted)
            .is_some()
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        1
    );

    // 1. Predicted component and confirmed component are different
    let tick = stepper.client_tick(0);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(Confirmed(CompFull(2.0)));
    // simulate that we received a server message for the confirmed entity on tick `tick`
    // where the PredictionHistory had the value of 1.0

    // step once to avoid 0 tick rollback
    stepper.frame_step(1);

    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        2
    );
    // the predicted history now has CompFull(2.0)

    // 2. Confirmed component does not exist but predicted component exists
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<Confirmed<CompFull>>();
    // simulate that we received a server message for the confirmed entity on tick `tick`
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        3
    );
    // the predicted history now has Absent

    // 3. Confirmed component exists but predicted component does not exist
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<CompFull>();
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(Confirmed(CompFull(2.0)));
    // simulate that we received a server message for the confirmed entity on tick `tick`
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        4
    );
    // the predicted history now has ConfirmedSyncModeFull(2.0)

    // 4. If confirmed component is the same value as what we have in the history for predicted component, we do not rollback
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<PredictionHistory<CompFull>>()
        .unwrap()
        .add_update(tick, CompFull(2.0));

    // simulate that we received a server message for the confirmed entity on tick `tick`
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);
    // no rollback
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        4
    );
}

/// Test that:
/// - we remove a component from the predicted entity
/// - rolling back before the remove should re-add it
///   We are still able to rollback properly (the rollback adds the component to the predicted entity)
#[test]
fn test_removed_predicted_component_rollback() {
    let (mut stepper, predicted) = setup();
    fn increment_component_system(
        mut commands: Commands,
        mut query_networked: Query<(Entity, &mut CompFull), With<Predicted>>,
    ) {
        for (entity, mut component) in query_networked.iter_mut() {
            component.0 += 1.0;
            if component.0 == 5.0 {
                commands.entity(entity).remove::<CompFull>();
            }
        }
    }
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component_system);
    stepper.frame_step(1);

    // check that the component got synced
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap(),
        &CompFull(2.0)
    );
    // also insert a non-networked component directly on the predicted entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(1.0));

    // advance five more frames, so that the component gets removed on predicted
    stepper.frame_step(5);

    // check that the networked component got removed on predicted
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_none()
    );
    // also remove the non-networked component
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<CompNotNetworked>();

    // create a rollback situation
    let tick = stepper.client_tick(0);
    info!("Trigger rollback back to {:?}", tick - 3);
    stepper
        .client_app()
        .world_mut()
        .get_mut::<Confirmed<CompFull>>(predicted)
        .unwrap()
        .0
        .0 = -10.0;
    trigger_rollback_check(&mut stepper, tick - 3);
    stepper.frame_step(1);

    // check that rollback happened
    // predicted got the component re-added and that we rolled back 3 ticks and advances by 1 tick
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .get_mut::<CompFull>(predicted)
            .unwrap()
            .0,
        -6.0
    );
    // the non-networked component got rolled back as well
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .get_mut::<CompNotNetworked>(predicted)
            .unwrap()
            .0,
        1.0
    );
}

/// Test that:
/// - a component gets added on Predicted
/// - we trigger a rollback, and the confirmed entity does not have the component
/// - the rollback removes the component from the predicted entity
#[test]
fn test_added_predicted_component_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);

    // the prediction history is updated with this tick
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // add a non-networked component as well, which should be removed on the rollback
    // since it did not exist at the rollback tick
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(1.0));

    // create a rollback situation to a tick where
    // - confirmed_component missing
    // - predicted component exists in history
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<Confirmed<CompFull>>();
    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // check that rollback happened:
    // the registered component got removed from predicted since it was not present on confirmed
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_none()
    );
    // the non-networked component got removed from predicted as it wasn't present in the history
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompNotNetworked>(predicted)
            .is_none()
    );
}

/// If we have disable_rollback:
/// 1) we don't check rollback for that entity
/// 2) if a rollback happens, we reset to the predicted history value instead of the confirmed value
#[test]
fn test_disable_rollback() {
    let (mut stepper, predicted_b) = setup();

    // add predicted/confirmed entities
    let receiver = stepper.client(0).id();
    let tick = stepper.client_tick(0);
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            Replicated { receiver },
            ConfirmedTick { tick },
            DeterministicPredicted,
            Confirmed(CompFull(1.0)),
        ))
        .id();

    // value gets synced and added to PredictionHistory
    stepper.frame_step(1);

    // 1. check rollback doesn't trigger on disable-rollback entities
    let tick = stepper.client_tick(0);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_a)
        .get_mut::<Confirmed<CompFull>>()
        .unwrap()
        .0
        .0 = 2.0;
    // simulate that we received a server message for the confirmed entity on tick `tick`
    trigger_rollback_check(&mut stepper, tick);
    let num_rollbacks = stepper
        .client_app()
        .world()
        .resource::<PredictionMetrics>()
        .rollbacks;
    stepper.frame_step(1);
    // no rollback
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        num_rollbacks
    );

    // 2. If a rollback happens, then we reset DisableRollback entities to their historical value
    stepper.frame_step(1);
    let tick = stepper.client_tick(0);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_b)
        .get_mut::<Confirmed<CompFull>>()
        .unwrap()
        .0
        .0 = 3.0;
    let mut history = PredictionHistory::<CompFull>::default();
    history.add_update(tick, CompFull(10.0));
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_a)
        .insert(history);
    // step once to avoid a 0-tick rollback
    stepper.frame_step(1);
    // simulate that we received a server message for the confirmed entities on tick `tick`
    // (all predicted entities are in the same ReplicationGroup)
    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);

    // the DisableRollback entity was rolledback to the past PredictionHistory value
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

fn setup_stepper_for_input_rollback(
    mode: RollbackMode,
) -> (ClientServerStepper, Entity, Entity, Entity, Entity) {
    let mut stepper = ClientServerStepper::with_clients(3);

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

    // on client 0, add DeterministicPredicted so that we can receive remote input messages
    let client_entity_a = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .expect("entity was not replicated to client");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity_a)
        .insert((DeterministicPredicted, CompNotNetworked(1.0)));
    let client_entity_b = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity_b)
        .insert(DeterministicPredicted);

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
        move |manager: Single<(&LocalTimeline, &LastConfirmedInput, &PredictionManager)>| {
            let (timeline, last_confirmed_input, manager) = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(last_confirmed_input.received_input());
                let rollback_start = manager.get_rollback_start_tick();
                // We receive the input message for tick `input_tick`, but we rollback at the previous LastConfirmedInput tick,
                // which is `input_tick - 1`
                assert_eq!(rollback_start.unwrap(), input_tick - 1,);
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSet::Check)
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

    let check_rollback_start = move |manager: Single<(&LocalTimeline, &PredictionManager)>| {
        let (timeline, manager) = manager.into_inner();
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
            .after(RollbackSet::Check)
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

    let check_rollback_start = move |manager: Single<(&LocalTimeline, &PredictionManager)>| {
        let (timeline, manager) = manager.into_inner();
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
            .after(RollbackSet::Check)
            .before(RollbackSet::Prepare),
    );

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);
}
