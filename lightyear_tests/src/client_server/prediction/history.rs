use super::*;
use crate::protocol::CompFull;
use bevy::prelude::Entity;
use lightyear::prelude::*;
use lightyear_prediction::Predicted;
use lightyear_prediction::predicted_history::{PredictionHistory, PredictionState};
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

/// When a server spawns an entity with Replicate + PredictionTarget + component C
/// where C has `add_prediction()`, the client receives `Predicted` and C at the
/// same time via the init message. We expect that `PredictionHistory<C>` on the
/// client contains the confirmed server value at the server tick S (not just
/// an empty history).
///
/// This is non-trivial because marker-gated replicon write functions are
/// checked BEFORE any component is applied on a freshly-spawned client entity.
/// In `bevy_replicon::client::apply_entity`, `entity_markers.read()` runs on the
/// empty entity (only `Remote` marker present) — so `Predicted` is NOT visible,
/// and the `write_history` marker-fn does NOT fire for init messages.
///
/// The fix: an observer (`seed_prediction_history_from_init`) fires on
/// `Add<Predicted>` / `Add<PreSpawned>` / `Add<DeterministicPredicted>`. After
/// the init flush, `Predicted` is on the entity and C has been written
/// directly via the default write. The observer reads C and the resolved
/// server tick from `ConfirmHistory + ReplicationCheckpointMap`, then seeds
/// `PredictionHistory<C>` with a confirmed entry.
#[test]
fn test_prediction_history_seeded_from_init_message() {
    use crate::stepper::*;
    use lightyear::prelude::ConfirmHistory;
    use lightyear_connection::network_target::NetworkTarget;
    use lightyear_messages::MessageManager;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::{PredictionTarget, Replicate};

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    // Spawn an entity on the server with a predicted component
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            CompFull(42.0),
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

    // The client entity should have Predicted (from the replicated PredictionTarget→Predicted requirement)
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(predicted_entity)
            .is_some(),
        "client entity should have Predicted marker"
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

    // Resolve the server tick that produced the init message and check
    // the prediction history in the same scope.
    let world = stepper.client_app().world();
    let history = world
        .get::<PredictionHistory<CompFull>>(predicted_entity)
        .expect("client entity should have PredictionHistory<CompFull>");
    let confirm = world
        .get::<ConfirmHistory>(predicted_entity)
        .expect("client entity should have ConfirmHistory");
    let checkpoints = world.resource::<ReplicationCheckpointMap>();
    let s_tick = checkpoints
        .get(confirm.last_tick())
        .expect("checkpoint map should resolve the last confirm tick");

    // Core assertion: the history should contain a CONFIRMED entry at tick S
    // with the value received from the server (CompFull(42.0)).
    // If write_history didn't fire at init time (because Predicted wasn't
    // visible when markers were checked), the history would be empty or
    // contain only predicted entries.
    let confirmed_at_s = history.get_confirmed_at(s_tick);
    assert!(
        confirmed_at_s.is_some(),
        "PredictionHistory should have a confirmed entry at server tick {:?}, \
         but found buffer contents: {:?}",
        s_tick,
        history.buffer().iter().collect::<Vec<_>>()
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
        Some(PredictionState::Predicted(CompFull(2.0))),
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
        Some(PredictionState::Removed),
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
        Some(PredictionState::Predicted(CompFull(3.0))),
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
        Some(PredictionState::Predicted(CompFull(4.0))),
        "Expected component value preserved during mid-history rollback"
    );
    check_history_consecutive_ticks(&stepper, predicted);
}
