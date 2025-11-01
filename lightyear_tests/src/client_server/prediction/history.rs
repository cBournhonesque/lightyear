use super::*;
use crate::protocol::CompFull;
use bevy::prelude::Entity;
use lightyear::prelude::*;
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
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
            .get::<HistoryBuffer<CompFull>>(predicted)
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
            .get::<HistoryBuffer<CompFull>>(predicted)
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
        .spawn((
            Predicted,
            Replicated {
                receiver: Entity::PLACEHOLDER,
            },
            ConfirmedTick { tick },
        ))
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

    // 3. Updating Comp::Full on confirmed entity during rollback
    let rollback_tick = Tick(10);
    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(Confirmed(CompFull(3.0)));
    info!(
        "Inserted CompFull(3.0) during rollback at tick {:?}",
        rollback_tick
    );
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
        "Expected component value to be updated in prediction history"
    );
    check_history_consecutive_ticks(&stepper, predicted);

    // 4. Updating Comp::Full on confirmed entity for a tick that is in the middle of the history
    // Previous test cases had the rollback tick be earlier than the entire history; we also need to test
    // when the rollback tick is in the middle of the history
    trigger_state_rollback(&mut stepper, rollback_tick + 3);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(Confirmed(CompFull(2.0)));
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(rollback_tick + 3),
        Some(HistoryState::Updated(CompFull(2.0))),
        "Expected component value to be updated in prediction history"
    );
    check_history_consecutive_ticks(&stepper, predicted);

    // 5. Removing Comp::Full on predicted entity during rollback
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<Confirmed<CompFull>>();
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
        Some(HistoryState::Removed),
        "Expected component value to be removed from prediction history"
    );
}
