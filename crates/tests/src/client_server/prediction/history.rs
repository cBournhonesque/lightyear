use super::*;
use crate::protocol::{CompA, CompCorr, CompFull};
use bevy::prelude::{Add, Commands, Component, Entity, On, Query, With};
use lightyear::prelude::*;
use lightyear_core::history_buffer::HistoryState;
use lightyear_prediction::Predicted;
use lightyear_prediction::predicted_history::PredictionHistory;
use test_log::test;

#[test]
fn test_history_added_when_prespawned_added() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let predicted = stepper.client_app().world_mut().spawn(CompFull(1.0)).id();
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(predicted)
            .is_none()
    );
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(PreSpawned::new(0));
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(predicted)
            .is_some()
    );
}

// TODO: test that PredictionHistory is added when a component is added to a PrePredicted or PreSpawned entity

/// Incoming `PredictedSend` is processed before the other components in the
/// initial update, so every predicted component is inserted live and into
/// confirmed history before observers run.
#[test]
fn test_prediction_history_received_from_initial_marker() {
    use crate::stepper::*;
    use lightyear::prelude::ConfirmHistory;
    use lightyear_connection::network_target::NetworkTarget;
    use lightyear_messages::MessageManager;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::{PredictionTarget, Replicate};

    #[derive(Component)]
    struct ObservedCompleteInitialPrediction;

    fn observe_initial_prediction(
        trigger: On<Add, Predicted>,
        query: Query<
            (),
            (
                With<CompFull>,
                With<CompCorr>,
                With<ConfirmedHistory<CompFull>>,
                With<ConfirmedHistory<CompCorr>>,
            ),
        >,
        mut commands: Commands,
    ) {
        if query.contains(trigger.entity) {
            commands
                .entity(trigger.entity)
                .insert(ObservedCompleteInitialPrediction);
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper
        .client_app()
        .add_observer(observe_initial_prediction);

    // Spawn an entity on the server with a predicted component
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            CompFull(42.0),
            CompCorr(24.0),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();

    // Let the entity replicate to the client
    stepper.frame_step(2);

    let predicted_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");

    // The client entity should have Predicted after receiving PredictedSend.
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(predicted_entity)
            .is_some(),
        "client entity should have Predicted marker"
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(predicted_entity)
            .contains::<ObservedCompleteInitialPrediction>(),
        "Predicted observers should see live components and confirmed histories from the same update"
    );

    // The client entity should have the CompFull value from the server
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_entity)
            .expect("client entity should have CompFull from replication"),
        &CompFull(42.0)
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompCorr>(predicted_entity)
            .expect("client entity should have CompCorr from replication"),
        &CompCorr(24.0)
    );

    // Resolve the server tick that produced the init message and check
    // the confirmed history in the same scope.
    let world = stepper.client_app().world();
    let prediction_history = world
        .get::<PredictionHistory<CompFull>>(predicted_entity)
        .expect("client entity should have PredictionHistory<CompFull>");
    let confirmed_history = world
        .get::<ConfirmedHistory<CompFull>>(predicted_entity)
        .expect("client entity should have ConfirmedHistory<CompFull>");
    let corr_confirmed_history = world
        .get::<ConfirmedHistory<CompCorr>>(predicted_entity)
        .expect("client entity should have ConfirmedHistory<CompCorr>");
    let confirm = world
        .get::<ConfirmHistory>(predicted_entity)
        .expect("client entity should have ConfirmHistory");
    let checkpoints = world.resource::<ReplicationCheckpointMap>();
    let s_tick = checkpoints
        .get(confirm.last_tick())
        .expect("checkpoint map should resolve the last confirm tick");

    // Core assertion: the confirmed history should contain an entry at tick S
    // with the value received from the server (CompFull(42.0)).
    // If write_history didn't fire at init time (because Predicted wasn't
    // visible when markers were checked), the confirmed history would be empty.
    let confirmed_at_s = confirmed_history.get_state_at(s_tick);
    assert!(
        confirmed_at_s.is_some(),
        "ConfirmedHistory should have a confirmed entry at server tick {:?}, \
         but found history: {:?}",
        s_tick,
        confirmed_history
    );
    assert_eq!(
        corr_confirmed_history
            .get_state_at(s_tick)
            .and_then(HistoryState::value),
        Some(&CompCorr(24.0)),
        "all initially replicated predicted components should use marker receive functions"
    );
    assert!(
        prediction_history.buffer().iter().all(|(_, state)| {
            matches!(state, HistoryState::Updated(_) | HistoryState::Removed)
        }),
        "PredictionHistory should contain only local predicted states: {:?}",
        prediction_history
    );
}

#[test]
fn test_manual_predicted_marker_backfills_existing_replicated_component() {
    use lightyear::prelude::ConfirmHistory;
    use lightyear_connection::network_target::NetworkTarget;
    use lightyear_messages::MessageManager;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::Replicate;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((CompFull(42.0), Replicate::to_clients(NetworkTarget::All)))
        .id();
    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");
    assert!(
        stepper
            .client_app()
            .world()
            .get::<ConfirmedHistory<CompFull>>(client_entity)
            .is_none()
    );

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(Predicted);

    let world = stepper.client_app().world();
    let confirm_tick = world
        .get::<ConfirmHistory>(client_entity)
        .expect("replicated entity should have ConfirmHistory")
        .last_tick();
    let server_tick = world
        .resource::<ReplicationCheckpointMap>()
        .get(confirm_tick)
        .expect("confirmation tick should resolve to a server tick");
    assert!(
        world
            .get::<PredictionHistory<CompFull>>(client_entity)
            .is_some(),
        "manual prediction should initialize local prediction history"
    );
    assert_eq!(
        world
            .get::<ConfirmedHistory<CompFull>>(client_entity)
            .and_then(|history| history.get_state_at(server_tick))
            .and_then(HistoryState::value),
        Some(&CompFull(42.0)),
        "manual prediction should seed confirmed history from the existing replicated value"
    );
    assert_eq!(world.get::<CompFull>(client_entity), Some(&CompFull(42.0)));
}

#[test]
fn test_manual_predicted_and_component_does_not_seed_confirmed_history() {
    use lightyear_connection::network_target::NetworkTarget;
    use lightyear_messages::MessageManager;
    use lightyear_replication::prelude::Replicate;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((CompA(1.0), Replicate::to_clients(NetworkTarget::All)))
        .id();
    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert((Predicted, CompFull(99.0)));

    let entity = stepper.client_app().world().entity(client_entity);
    assert!(entity.contains::<PredictionHistory<CompFull>>());
    assert!(
        !entity.contains::<ConfirmedHistory<CompFull>>(),
        "a component added locally with Predicted is not authoritative"
    );
}

/// A Full-mode component inserted by the server after the predicted entity was
/// spawned should be detected as a mismatch and applied through rollback.
#[test]
fn test_server_insert_on_existing_predicted_entity_triggers_rollback() {
    use bevy::prelude::*;
    use lightyear_messages::MessageManager;
    use lightyear_replication::prelude::{PredictionTarget, Replicate};

    #[derive(Resource, Default)]
    struct RollbackObserved(bool);

    fn observe_rollback(manager: Res<PredictionManager>, mut observed: ResMut<RollbackObserved>) {
        observed.0 |= manager.is_rollback();
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_app().init_resource::<RollbackObserved>();
    stepper.client_app().add_systems(
        PreUpdate,
        observe_rollback
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(2);
    let predicted_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to the client");
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_entity)
            .is_none()
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompFull(42.0));
    stepper.frame_step(2);

    let world = stepper.client_app().world();
    assert!(
        world.resource::<RollbackObserved>().0,
        "the completed-checkpoint full scan should detect the authoritative insert"
    );
    assert_eq!(
        world
            .get::<ConfirmedHistory<CompFull>>(predicted_entity)
            .and_then(ConfirmedHistory::newest_present)
            .map(|(_, value)| value),
        Some(&CompFull(42.0)),
        "the authoritative insert should be buffered in confirmed history"
    );
    assert!(
        world
            .get::<PredictionHistory<CompFull>>(predicted_entity)
            .is_some(),
        "the confirmed insert should seed rollback membership for the component"
    );

    assert_eq!(
        world.get::<CompFull>(predicted_entity),
        Some(&CompFull(42.0)),
        "the authoritative insert should trigger a rollback that applies the component"
    );
}

/// When two previously absent components arrive in the same update, the first mismatch suppresses
/// the redundant check for the second. Both still need prediction history so the resulting rollback
/// restores both components.
#[test]
fn test_two_server_inserts_are_both_applied_by_one_rollback() {
    use bevy::prelude::*;
    use lightyear_messages::MessageManager;
    use lightyear_replication::prelude::{PredictionTarget, Replicate};

    #[derive(Resource, Default)]
    struct RollbackObserved(bool);

    fn observe_rollback(manager: Res<PredictionManager>, mut observed: ResMut<RollbackObserved>) {
        observed.0 |= manager.is_rollback();
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_app().init_resource::<RollbackObserved>();
    stepper.client_app().add_systems(
        PreUpdate,
        observe_rollback
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(2);
    let predicted_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to the client");
    let client_entity = stepper.client_app().world().entity(predicted_entity);
    assert!(!client_entity.contains::<CompFull>());
    assert!(!client_entity.contains::<CompCorr>());

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert((CompFull(42.0), CompCorr(24.0)));
    stepper.frame_step(2);

    let world = stepper.client_app().world();
    assert!(
        world.resource::<RollbackObserved>().0,
        "the authoritative inserts should trigger a rollback"
    );
    assert!(
        world
            .get::<PredictionHistory<CompFull>>(predicted_entity)
            .is_some(),
        "the CompFull insert should seed rollback membership"
    );
    assert!(
        world
            .get::<PredictionHistory<CompCorr>>(predicted_entity)
            .is_some(),
        "the CompCorr insert should seed rollback membership even if its mismatch check was suppressed"
    );
    assert_eq!(
        world.get::<CompFull>(predicted_entity),
        Some(&CompFull(42.0))
    );
    assert_eq!(
        world.get::<CompCorr>(predicted_entity),
        Some(&CompCorr(24.0))
    );
}

/// Test that the history gets updated correctly
/// 1. Updating the predicted component for Comp::Full
/// 2. Removing the predicted component
/// 3. Updating the predicted component during rollback
/// 4. Removing the predicted component during rollback
#[test]
fn test_update_history() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    fn check_history_consecutive_ticks(stepper: &ClientServerStepper, entity: Entity) {
        let history = stepper.client_apps[0]
            .world()
            .get::<PredictionHistory<CompFull>>(entity)
            .expect("Expected prediction history to be added");
        let mut last_tick: Option<Tick> = None;
        for (tick, _) in history.buffer().iter() {
            if let Some(last) = last_tick {
                assert_eq!(
                    tick.0,
                    *last + 1,
                    "History has duplicate or out-of-order ticks"
                );
            }
            last_tick = Some(*tick);
        }
    }

    // add predicted, component
    let tick = stepper.client_tick(0);
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, Replicated))
        .id();

    // 1. Updating Comp::Full on predicted component
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompFull(1.0));
    stepper.frame_step(1);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<CompFull>()
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(1);
    let tick = stepper.client_tick(0);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(tick),
        Some(HistoryState::Updated(CompFull(2.0))),
        "Expected component value to be updated in prediction history"
    );

    // 2. Removing Comp::Full on predicted entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<CompFull>();
    stepper.frame_step(1);
    let tick = stepper.client_tick(0);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(tick),
        Some(HistoryState::Removed),
        "Expected component value to be removed in prediction history"
    );

    // 3. After rollback, component is restored from history
    // Re-add CompFull and build history so rollback has valid data
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompFull(3.0));
    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1); // advance so there's room for rollback
    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(rollback_tick),
        Some(HistoryState::Updated(CompFull(3.0))),
        "Expected component value to be restored from history during rollback"
    );
    check_history_consecutive_ticks(&stepper, predicted);

    // 4. Rollback to middle of history preserves the value at that tick
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<CompFull>()
        .unwrap()
        .0 = 4.0;
    stepper.frame_step(1);
    let mid_tick = stepper.client_tick(0);
    stepper.frame_step(1);
    trigger_state_rollback(&mut stepper, mid_tick);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(mid_tick),
        Some(HistoryState::Updated(CompFull(4.0))),
        "Expected component value preserved during mid-history rollback"
    );
    check_history_consecutive_ticks(&stepper, predicted);
}
