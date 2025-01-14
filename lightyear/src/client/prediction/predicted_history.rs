//! Managed the history buffer, which is a buffer of the past predicted component states,
//! so that whenever we receive an update from the server we can compare the predicted entity's history with the server update.
use std::ops::Deref;

use bevy::prelude::{
    Added, Commands, Component, DetectChanges, Entity, OnRemove, Or, Query, Ref, Res, Trigger,
    With, Without,
};
use tracing::{debug, trace};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::prediction::history::HistoryBuffer;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::prelude::{ComponentRegistry, PreSpawnedPlayerObject, ShouldBePredicted, TickManager};
use crate::shared::tick_manager::{Tick, TickEvent};

pub(crate) type PredictionHistory<C> = HistoryBuffer<C>;

// TODO: should this be handled with observers? to avoid running a system
//  for something that happens relatively rarely
/// System that adds a `PredictedHistory` for rollback components that
/// were added to a Predicted entity, but are not networked
pub(crate) fn add_non_networked_component_history<C: Component + PartialEq + Clone>(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    predicted_entities: Query<
        (Entity, &C),
        (
            Without<PredictionHistory<C>>,
            // for all types of predicted entities, we want to add the component history to enable them to be rolled-back
            With<Predicted>,
            Added<C>,
        ),
    >,
) {
    let tick = tick_manager.tick();
    for (entity, predicted_component) in predicted_entities.iter() {
        let mut history = PredictionHistory::<C>::default();
        history.add_update(tick, predicted_component.clone());
        commands.entity(entity).insert(history);
    }
}

/// Add component history for entities that are predicted
/// There is extra complexity because the component could get added on the Confirmed entity (received from the server), or added to the Predicted entity directly
/// Also:
/// - handle PrePredicted entities (the Predicted entity might already have the component)
/// - handle entity that become Predicted after authority transfer (the Confirmed entity might already have the component)
#[allow(clippy::type_complexity)]
pub(crate) fn add_component_history<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    manager: Res<PredictionManager>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    predicted_components: Query<
        Option<Ref<C>>,
        (
            Without<PredictionHistory<C>>,
            // for all types of predicted entities, we want to add the component history to enable them to be rolled-back
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(Entity, Ref<Confirmed>, Option<Ref<C>>)>,
) {
    let kind = std::any::type_name::<C>();
    let tick = tick_manager.tick();
    for (confirmed_entity, confirmed, confirmed_component) in confirmed_entities.iter() {
        if let Some(predicted_entity) = confirmed.predicted {
            if let Ok(predicted_component) = predicted_components.get(predicted_entity) {
                // if component got added on predicted side, add history
                add_history::<C>(
                    component_registry.as_ref(),
                    tick,
                    predicted_entity,
                    &predicted_component,
                    &mut commands,
                );

                // if component got added on confirmed side, or if confirmed itself got added (this is useful to handle cases
                // where the confirmed entity already exists and had the component, for example when authority was transferred
                // away from the client and the client needs to add prediction)
                // - full: sync component and add history
                // - simple/once: sync component
                if let Some(confirmed_component) = confirmed_component {
                    if confirmed_component.is_added() || confirmed.is_added() {
                        trace!(?kind, "Component added on confirmed side");
                        // safety: we know the entity exists
                        let mut predicted_entity_mut =
                            commands.get_entity(predicted_entity).unwrap();
                        // map any entities from confirmed to predicted
                        let mut new_component = confirmed_component.deref().clone();
                        let _ =
                            manager.map_entities(&mut new_component, component_registry.as_ref());
                        match component_registry.prediction_mode::<C>() {
                            ComponentSyncMode::Full => {
                                debug!("Adding history for {:?}", std::any::type_name::<C>());
                                // insert history, no need to add any value to it because we run the UpdateHistory system set after the SpawnHistory
                                // it will be quickly filled by a rollback (since it starts empty before the current client tick)
                                // or will it? because the component just got spawned anyway..
                                // TODO: then there's no need to add the component here, since it's going to get added during rollback anyway?
                                let history = PredictionHistory::<C>::default();
                                predicted_entity_mut.insert((new_component, history));
                            }
                            ComponentSyncMode::Simple => {
                                debug!(
                                    ?kind,
                                    "Component simple synced between confirmed and predicted"
                                );
                                // we only sync the components once, but we don't do rollback so no need for a component history
                                predicted_entity_mut.insert(new_component);
                            }
                            ComponentSyncMode::Once => {
                                // if this was a prespawned entity, don't override SyncMode::Once components!
                                if predicted_component.is_none() {
                                    // we only sync the components once, but we don't do rollback so no need for a component history
                                    predicted_entity_mut.insert(new_component);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// Add the history for prespawned entities.
/// This must run on FixedUpdate (for entities spawned on FixedUpdate) and PreUpdate (for entities spawned on Update)
#[allow(clippy::type_complexity)]
pub(crate) fn add_prespawned_component_history<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    prespawned_query: Query<
        (Entity, Ref<C>),
        (
            Without<PredictionHistory<C>>,
            Without<Confirmed>,
            // for pre-spawned entities
            Or<(With<ShouldBePredicted>, With<PreSpawnedPlayerObject>)>,
        ),
    >,
) {
    let tick = tick_manager.tick();
    // add component history for pre-spawned entities right away
    for (predicted_entity, predicted_component) in prespawned_query.iter() {
        trace!(
            ?tick,
            "Potentially adding prediction history for component {:?} for pre-spawned entity {:?}",
            std::any::type_name::<C>(),
            predicted_entity
        );
        add_history::<C>(
            component_registry.as_ref(),
            tick,
            predicted_entity,
            &Some(predicted_component),
            &mut commands,
        );
    }
}

/// Add a PredictionHistory component to the predicted entity
fn add_history<C: SyncComponent>(
    component_registry: &ComponentRegistry,
    tick: Tick,
    predicted_entity: Entity,
    predicted_component: &Option<Ref<C>>,
    commands: &mut Commands,
) {
    let kind = std::any::type_name::<C>();
    if component_registry.prediction_mode::<C>() == ComponentSyncMode::Full {
        if let Some(predicted_component) = predicted_component {
            // component got added on predicted side, add history
            if predicted_component.is_added() {
                debug!(?kind, ?tick, ?predicted_entity, "Adding prediction history");
                // insert history component
                // no need to add any value to it because we run the UpdateHistory system set after the SpawnHistory
                let history = PredictionHistory::<C>::default();
                commands.entity(predicted_entity).insert(history);
            }
        }
    }
}

/// If ComponentSyncMode::Full, we store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + PartialEq + Clone>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());

    // update history if the predicted component changed
    for (component, mut history) in query.iter_mut() {
        // change detection works even when running the schedule for rollback
        if component.is_changed() {
            history.add_update(tick, component.deref().clone());
        }
    }
}

/// If there is a TickEvent and the client tick suddenly changes, we need
/// to update the ticks in the history buffer.
///
/// The history buffer ticks are only relevant relative to the current client tick.
/// (i.e. X ticks in the past compared to the current tick)
pub(crate) fn handle_tick_event_prediction_history<C: Component>(
    trigger: Trigger<TickEvent>,
    mut query: Query<&mut PredictionHistory<C>>,
) {
    match *trigger.event() {
        TickEvent::TickSnap { old_tick, new_tick } => {
            for mut history in query.iter_mut() {
                history.update_ticks(new_tick - old_tick);
            }
        }
    }
}

/// If a component is removed on the Predicted entity, and the ComponentSyncMode == FULL
/// Add the removal to the history (for potential rollbacks)
pub(crate) fn apply_component_removal_predicted<C: Component + PartialEq + Clone>(
    trigger: Trigger<OnRemove, C>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
) {
    // TODO: do not run this if component-sync-mode != FULL
    // if the component was removed from the Predicted entity, add the Removal to the history
    if let Ok(mut history) = predicted_query.get_mut(trigger.entity()) {
        // tick for which we will record the history (either the current client tick or the current rollback tick)
        let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());
        history.add_remove(tick);
    }
}

/// If the component was removed from the Confirmed entity:
/// - if the ComponentSyncMode == ONCE, do nothing (we only care about replicating the component once)
/// - if the ComponentSyncMode == SIMPLE, remove the component from the Predicted entity
/// - if the ComponentSyncMode == FULL, do nothing. We might get a rollback by comparing with the history.
pub(crate) fn apply_component_removal_confirmed<C: SyncComponent>(
    trigger: Trigger<OnRemove, C>,
    mut commands: Commands,
    confirmed_query: Query<&Confirmed>,
) {
    // Components that are removed from the Confirmed entity also get removed from the Predicted entity
    if let Ok(confirmed) = confirmed_query.get(trigger.entity()) {
        if let Some(p) = confirmed.predicted {
            if let Some(mut commands) = commands.get_entity(p) {
                commands.remove::<C>();
            }
        }
    }
}

/// If ComponentSyncMode == Simple, when we receive a server update we want to apply it to the predicted entity
#[allow(clippy::type_complexity)]
pub(crate) fn apply_confirmed_update<C: SyncComponent>(
    component_registry: Res<ComponentRegistry>,
    manager: Res<PredictionManager>,
    mut predicted_entities: Query<
        &mut C,
        (
            Without<PredictionHistory<C>>,
            Without<Confirmed>,
            With<Predicted>,
        ),
    >,
    confirmed_entities: Query<(&Confirmed, Ref<C>)>,
) {
    for (confirmed_entity, confirmed_component) in confirmed_entities.iter() {
        if let Some(p) = confirmed_entity.predicted {
            if confirmed_component.is_changed() && !confirmed_component.is_added() {
                if let Ok(mut predicted_component) = predicted_entities.get_mut(p) {
                    assert_eq!(
                        component_registry.prediction_mode::<C>(),
                        ComponentSyncMode::Simple
                    );
                    // map any entities from confirmed to predicted
                    let mut component = confirmed_component.deref().clone();
                    let _ = manager.map_entities(&mut component, component_registry.as_ref());
                    *predicted_component = component;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::prediction::history::HistoryState;
    use crate::prelude::client::RollbackState;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::ecs::system::RunSystemOnce;

    /// Test adding the component history to the predicted entity
    /// 1. Add the history for ComponentSyncMode::Full that was added to the confirmed entity
    /// 2. Add the history for ComponentSyncMode::Full that was added to the predicted entity
    /// 3. Don't add the history for ComponentSyncMode::Simple
    /// 4. Don't add the history for ComponentSyncMode::Once
    /// 5. For components that have MapEntities, the component gets mapped from Confirmed to Predicted
    #[test]
    fn test_add_component_history() {
        let mut stepper = BevyStepper::default();

        let tick = stepper.client_tick();
        let confirmed = stepper
            .client_app
            .world_mut()
            .spawn(Confirmed {
                tick,
                ..Default::default()
            })
            .id();
        let predicted = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed),
            })
            .id();

        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted);

        // 1. Add the history for ComponentSyncMode::Full that was added to the confirmed entity
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeFull(1.0));
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Updated(ComponentSyncModeFull(1.0))),
            "Expected component value to be added to prediction history"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeFull>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeFull(1.0),
            "Expected component to be added to predicted entity"
        );

        // 2. Add the history for ComponentSyncMode::Full that was added to the predicted entity
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentSyncModeFull2(1.0));
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull2>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Updated(ComponentSyncModeFull2(1.0))),
            "Expected component value to be added to prediction history"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeFull2>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeFull2(1.0),
            "Expected component to be added to predicted entity"
        );

        // 3. Don't add the history for ComponentSyncMode::Simple
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeSimple(1.0));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<PredictionHistory<ComponentSyncModeSimple>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Simple"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeSimple>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeSimple(1.0),
            "Expected component to be added to predicted entity"
        );

        // 4. Don't add the history for ComponentSyncMode::Once
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeOnce(1.0));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world()                .entity(predicted)
                .get::<PredictionHistory<ComponentSyncModeOnce>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Once"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeOnce>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeOnce(1.0),
            "Expected component to be added to predicted entity"
        );

        // 5. Component with MapEntities get mapped from Confirmed to Predicted
        let tick = stepper.client_tick();
        stepper
            .client_app
            .world_mut()
            .resource_mut::<PredictionManager>()
            .predicted_entity_map
            .get_mut()
            .confirmed_to_predicted
            .insert(confirmed, predicted);
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentMapEntities(confirmed));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<PredictionHistory<ComponentMapEntities>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Simple"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentMapEntities>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentMapEntities(predicted),
            "Expected component to be added to predicted entity with entity mapping"
        );
    }

    /// Test that the history gets updated correctly
    /// 1. Updating the predicted component for ComponentSyncMode::Full
    /// 2. Updating the confirmed component for ComponentSyncMode::Simple
    /// 3. Removing the predicted component
    /// 4. Removing the confirmed component
    /// 5. Updating the predicted component during rollback
    /// 6. Removing the predicted component during rollback
    #[test]
    fn test_update_history() {
        let mut stepper = BevyStepper::default();

        // add predicted, component
        let tick = stepper.client_tick();
        let confirmed = stepper
            .client_app
            .world_mut()
            .spawn(Confirmed {
                tick,
                ..Default::default()
            })
            .id();
        let predicted = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed),
            })
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<Confirmed>()
            .unwrap()
            .predicted = Some(predicted);

        // 1. Updating ComponentSyncMode::Full on predicted component
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeFull(1.0));
        stepper.frame_step();
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .get_mut::<ComponentSyncModeFull>()
            .unwrap()
            .0 = 2.0;
        stepper.frame_step();
        let tick = stepper.client_tick();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Updated(ComponentSyncModeFull(2.0))),
            "Expected component value to be updated in prediction history"
        );

        // 2. Updating ComponentSyncMode::Simple on confirmed entity
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeSimple(1.0));
        stepper.frame_step();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .get_mut::<ComponentSyncModeSimple>()
            .unwrap()
            .0 = 2.0;
        let tick = stepper.client_tick();
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeSimple>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeSimple(2.0),
            "Expected ComponentSyncMode::Simple component to be updated in predicted entity"
        );

        // 3. Removing ComponentSyncMode::Full on predicted entity
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .remove::<ComponentSyncModeFull>();
        stepper.frame_step();
        let tick = stepper.client_tick();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Removed),
            "Expected component value to be removed in prediction history"
        );

        // 4. Removing ComponentSyncMode::Simple on confirmed entity
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .remove::<ComponentSyncModeSimple>();
        let tick = stepper.client_tick();
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentSyncModeSimple>()
                .is_none(),
            "Expected component value to be removed from predicted entity"
        );

        // 5. Updating ComponentSyncMode::Full on predicted entity during rollback
        let rollback_tick = Tick(10);
        stepper
            .client_app
            .world_mut()
            .insert_resource(Rollback::new(RollbackState::ShouldRollback {
                current_tick: rollback_tick,
            }));
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentSyncModeFull(3.0));
        let _ = stepper
            .client_app
            .world_mut()
            .run_system_once(update_prediction_history::<ComponentSyncModeFull>);
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(rollback_tick),
            Some(HistoryState::Updated(ComponentSyncModeFull(3.0))),
            "Expected component value to be updated in prediction history"
        );

        // 6. Removing ComponentSyncMode::Full on predicted entity during rollback
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .remove::<ComponentSyncModeFull>();
        let _ = stepper
            .client_app
            .world_mut()
            .run_system_once(update_prediction_history::<ComponentSyncModeFull>);
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentSyncModeFull>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(rollback_tick),
            Some(HistoryState::Removed),
            "Expected component value to be removed from prediction history"
        );
    }
}
