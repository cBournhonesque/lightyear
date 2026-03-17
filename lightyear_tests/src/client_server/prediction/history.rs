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
