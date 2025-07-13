use super::*;
use crate::protocol::{CompCorr, CompFull, CompMap, CompOnce, CompSimple};
use bevy::prelude::{OnAdd, Trigger};
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_prediction::Predicted;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::PreSpawned;
use lightyear_replication::prelude::ShouldBePredicted;
use test_log::test;

#[test]
fn test_history_added_when_prespawned_added() {
    let mut stepper = ClientServerStepper::single();
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

/// Test that components are synced from Confirmed to Predicted and that PredictionHistory is
/// added correctly
///
/// 1. Sync Comp::Full added to the confirmed entity + history is added
/// 2. Add the history for Comp::Full that was added to the predicted entity
/// 3. Sync Comp::Once added to the confirmed entity but don't add history
/// 4. Sync Comp::Simple added to the confirmed entity but don't add history
/// 5. For components that have MapEntities, the component gets mapped from Confirmed to Predicted
/// 6. Sync pre-existing components when Confirmed is added to an entity
#[test]
fn test_confirmed_to_predicted_sync() {
    let mut stepper = ClientServerStepper::single();
    let tick = stepper.client_tick(0);
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn(Predicted {
            confirmed_entity: None,
        })
        .id();
    let confirmed = stepper
        .client_app()
        .world_mut()
        .spawn(Confirmed {
            tick,
            predicted: Some(predicted),
            ..Default::default()
        })
        .id();
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .get_mut::<Predicted>()
        .unwrap()
        .confirmed_entity = Some(confirmed);

    // 1. Add the history for Comp::Full that was added to the confirmed entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompFull(1.0));
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
        Some(HistoryState::Updated(CompFull(1.0))),
        "Expected component value to be added to prediction history"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompFull>()
            .expect("Expected component to be added to predicted entity"),
        &CompFull(1.0),
        "Expected component to be added to predicted entity"
    );

    // 2. Add the history for Comp::Full that was added to the predicted entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompCorr(2.0));
    stepper.frame_step(1);
    let tick = stepper.client_tick(0);
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<PredictionHistory<CompCorr>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(tick),
        Some(HistoryState::Updated(CompCorr(2.0))),
        "Expected component value to be added to prediction history"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompCorr>()
            .expect("Expected component to be added to predicted entity"),
        &CompCorr(2.0),
        "Expected component to be added to predicted entity"
    );

    // 3. Don't add the history for Comp::Simple
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompSimple(1.0));
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<PredictionHistory<CompSimple>>()
            .is_none(),
        "Expected component value to not be added to prediction history for Comp::Simple"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompSimple>()
            .expect("Expected component to be added to predicted entity"),
        &CompSimple(1.0),
        "Expected component to be added to predicted entity"
    );

    // 4. Don't add the history for Comp::Once
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompOnce(1.0));
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<PredictionHistory<CompOnce>>()
            .is_none(),
        "Expected component value to not be added to prediction history for Comp::Once"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompOnce>()
            .expect("Expected component to be added to predicted entity"),
        &CompOnce(1.0),
        "Expected component to be added to predicted entity"
    );

    // 5. Component with MapEntities get mapped from Confirmed to Predicted
    stepper
        .client_mut(0)
        .get_mut::<PredictionManager>()
        .unwrap()
        .predicted_entity_map
        .get_mut()
        .confirmed_to_predicted
        .insert(confirmed, predicted);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompMap(confirmed));
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompMap>()
            .expect("Expected component to be added to predicted entity"),
        &CompMap(predicted),
        "Expected component to be added to predicted entity with entity mapping"
    );

    // 6. Sync components that were present on the confirmed entity before Confirmed is added
    let confirmed_2 = stepper
        .client_app()
        .world_mut()
        .spawn((
            CompFull(1.0),
            CompSimple(1.0),
            CompOnce(1.0),
            CompMap(confirmed),
        ))
        .id();
    let predicted_2 = stepper
        .client_app()
        .world_mut()
        .spawn(Predicted {
            confirmed_entity: Some(confirmed_2),
        })
        .id();
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed_2)
        .insert(Confirmed {
            tick,
            predicted: Some(predicted_2),
            interpolated: None,
        });

    stepper.frame_step(1);
    let tick = stepper.client_tick(0);

    // check that the components were synced
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(predicted_2)
            .get_mut::<PredictionHistory<CompFull>>()
            .expect("Expected prediction history to be added")
            .pop_until_tick(tick),
        Some(HistoryState::Updated(CompFull(1.0))),
        "Expected component value to be added to prediction history"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted_2)
            .get::<CompFull>()
            .expect("Expected component to be added to predicted entity"),
        &CompFull(1.0),
        "Expected component to be added to predicted entity"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted_2)
            .get::<CompOnce>()
            .expect("Expected component to be added to predicted entity"),
        &CompOnce(1.0),
        "Expected component to be added to predicted entity"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted_2)
            .get::<CompMap>()
            .expect("Expected component to be added to predicted entity"),
        &CompMap(predicted),
        "Expected component to be added to predicted entity with entity mapping"
    );
}

// TODO: test that PredictionHistory is added when a component is added to a PrePredicted or PreSpawned entity

/// Test that components are synced from Confirmed to Predicted simultaneously, not sequentially
#[test]
fn test_predicted_sync_batch() {
    let mut stepper = ClientServerStepper::single();
    // make sure that when ComponentSimple is added, ComponentOnce was also added
    stepper.client_app().add_observer(
        |trigger: Trigger<OnAdd, CompSimple>, query: Query<(), With<CompOnce>>| {
            assert!(query.get(trigger.target()).is_ok());
        },
    );
    // make sure that when ComponentOnce is added, ComponentSimple was also added
    // i.e. both components are added at the same time
    stepper.client_app().add_observer(
        |trigger: Trigger<OnAdd, CompOnce>, query: Query<(), With<CompSimple>>| {
            assert!(query.get(trigger.target()).is_ok());
        },
    );

    stepper
        .client_app()
        .world_mut()
        .spawn((ShouldBePredicted, CompOnce(1.0), CompSimple(1.0)));
    stepper.frame_step(1);

    // check that the components were synced to the predicted entity
    assert!(
        stepper
            .client_app()
            .world_mut()
            .query_filtered::<(), (With<CompOnce>, With<CompSimple>, With<Predicted>)>()
            .single(stepper.client_app().world())
            .is_ok()
    );
}

/// Test that the history gets updated correctly
/// 1. Updating the predicted component for Comp::Full
/// 2. Updating the confirmed component for Comp::Simple
/// 3. Removing the predicted component
/// 4. Removing the confirmed component
/// 5. Updating the predicted component during rollback
/// 6. Removing the predicted component during rollback
#[test]
fn test_update_history() {
    let mut stepper = ClientServerStepper::single();

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
    let confirmed = stepper
        .client_app()
        .world_mut()
        .spawn(Confirmed {
            tick,
            ..Default::default()
        })
        .id();
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn(Predicted {
            confirmed_entity: Some(confirmed),
        })
        .id();
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .get_mut::<Confirmed>()
        .unwrap()
        .predicted = Some(predicted);

    // 1. Updating Comp::Full on predicted component
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
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

    // 2. Updating Comp::Simple on confirmed entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompSimple(1.0));
    stepper.frame_step(1);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .get_mut::<CompSimple>()
        .unwrap()
        .0 = 2.0;
    let tick = stepper.client_tick(0);
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompSimple>()
            .expect("Expected component to be added to predicted entity"),
        &CompSimple(2.0),
        "Expected Comp::Simple component to be updated in predicted entity"
    );

    // 3. Removing Comp::Full on predicted entity
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

    // 4. Removing Comp::Simple on confirmed entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .remove::<CompSimple>();
    let tick = stepper.client_tick(0);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .entity(predicted)
            .get::<CompSimple>()
            .is_none(),
        "Expected component value to be removed from predicted entity"
    );

    // 5. Updating Comp::Full on predicted entity during rollback
    let rollback_tick = Tick(10);
    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompFull(3.0));
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

    // 6. Updating Comp::Full on predicted entity for a tick that is in the middle of the history
    // Previous test cases had the rollback tick be earlier than the entire history; we also need to test
    // when the rollback tick is in the middle of the history
    trigger_state_rollback(&mut stepper, rollback_tick + 3);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .insert(CompFull(2.0));
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

    // 7. Removing Comp::Full on predicted entity during rollback
    stepper
        .client_app()
        .world_mut()
        .entity_mut(confirmed)
        .remove::<CompFull>();
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
