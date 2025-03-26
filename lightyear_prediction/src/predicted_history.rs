//! Managed the history buffer, which is a buffer of the past predicted component states,
//! so that whenever we receive an update from the server we can compare the predicted entity's history with the server update.
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::app::App;
use bevy::ecs::component::ComponentId;
use bevy::prelude::*;
use core::ops::Deref;

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::prelude::client::{Correction, PredictionSet};
use crate::prelude::{ComponentRegistry, HistoryBuffer, PrePredicted, PreSpawned, TickManager};
use crate::shared::tick_manager::TickEvent;

pub(crate) type PredictionHistory<C> = HistoryBuffer<C>;

/// If ComponentSyncMode::Full, we store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + Clone>(
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
    mut query: Query<(&mut PredictionHistory<C>, Option<&mut Correction<C>>)>,
) {
    match *trigger.event() {
        TickEvent::TickSnap { old_tick, new_tick } => {
            for (mut history, correction) in query.iter_mut() {
                history.update_ticks(new_tick - old_tick);
                if let Some(mut correction) = correction {
                    correction.update_ticks(new_tick - old_tick);
                }
            }
        }
    }
}

/// If a component is removed on the Predicted entity, and the ComponentSyncMode == FULL
/// Add the removal to the history (for potential rollbacks)
pub(crate) fn apply_component_removal_predicted<C: Component>(
    trigger: Trigger<OnRemove, C>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
) {
    // if the component was removed from the Predicted entity, add the Removal to the history
    if let Ok(mut history) = predicted_query.get_mut(trigger.target()) {
        // tick for which we will record the history (either the current client tick or the current rollback tick)
        let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());
        history.add_remove(tick);
    }
}

/// If the component was removed from the Confirmed entity:
/// - if the ComponentSyncMode == ONCE, do nothing (we only care about replicating the component once)
/// - if the ComponentSyncMode == SIMPLE, remove the component from the Predicted entity
/// - if the ComponentSyncMode == FULL, do nothing. We might get a rollback by comparing with the history.
pub(crate) fn apply_component_removal_confirmed<C: Component>(
    trigger: Trigger<OnRemove, C>,
    mut commands: Commands,
    confirmed_query: Query<&Confirmed>,
) {
    // Components that are removed from the Confirmed entity also get removed from the Predicted entity
    if let Ok(confirmed) = confirmed_query.get(trigger.target()) {
        if let Some(p) = confirmed.predicted {
            if let Ok(mut commands) = commands.get_entity(p) {
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

#[derive(Event, Debug)]
struct PredictedSyncEvent {
    confirmed: Entity,
    predicted: Entity,
    components: Vec<ComponentId>,
}

/// Sync components from confirmed entity to predicted entity
fn apply_predicted_sync(world: &mut World) {
    world.resource_scope(|world, mut events: Mut<Events<PredictedSyncEvent>>| {
        events.drain().for_each(|event| {
            // NOTE: we cannot use `world.resource_scope::<ComponentRegistry>` because doing the sync
            //  might trigger other Observers that might also use the ComponentRegistry
            //  Instead we'll use UnsafeWorldCell since the rest of the world does not modify the registry
            let unsafe_world = world.as_unsafe_world_cell();
            let mut component_registry =
                unsafe { unsafe_world.get_resource_mut::<ComponentRegistry>() }.unwrap();
            let world = unsafe { unsafe_world.world_mut() };
            // sync all components from the predicted to the confirmed entity and possibly add the PredictedHistory
            component_registry.batch_sync(
                &event.components,
                event.confirmed,
                event.predicted,
                world,
            );
        })
    });
}

/// If a ComponentSyncMode::Full gets added to [`PrePredicted`] or [`PreSpawned`] entity,
/// add a PredictionHistory component.
///
/// We don't put any value in the history because the `update_history` systems will add the value.
// TODO: We could not run this for [`Predicted`] entities and instead have the confirmed->sync observers already
//  add a PredictionHistory component if it's missing on the Predicted entity.
pub(crate) fn add_prediction_history<C: Component>(
    trigger: Trigger<OnAdd, C>,
    mut commands: Commands,
    // TODO: should we also have With<ShouldBePredicted>?
    query: Query<
        (),
        (
            Without<PredictionHistory<C>>,
            Or<(With<Predicted>, With<PrePredicted>, With<PreSpawned>)>,
        ),
    >,
) {
    if query.get(trigger.target()).is_ok() {
        commands
            .entity(trigger.target())
            .insert(PredictionHistory::<C>::default());
    }
}

/// When the Confirmed component is added, sync components to the Predicted entity
///
/// This is needed in two cases:
/// - when an entity is replicated, the components are replicated onto the Confirmed entity before the Confirmed
///   component is added
/// - when a client spawned on the client transfers authority to the server, the Confirmed
///   component can be added even though the entity already had existing components
///
/// We have some ordering constraints related to syncing hierarchy so we don't want to sync components
/// immediately here (because the ParentSync component might not be able to get mapped properly since the parent entity
/// might not be predicted yet). Therefore we send a PredictedSyncEvent so that all components can be synced at once.
fn confirmed_added_sync(
    trigger: Trigger<OnInsert, Confirmed>,
    confirmed_query: Query<EntityRef>,
    component_registry: Res<ComponentRegistry>,
    events: Option<ResMut<Events<PredictedSyncEvent>>>,
) {
    // `events` is None while we are inside the `apply_predicted_sync` system
    // that shouldn't be an issue because the components are being inserted only on Predicted entities
    // so we don't want to react to them
    let Some(mut events) = events else { return };
    let confirmed = trigger.target();
    let entity_ref = confirmed_query.get(confirmed).unwrap();
    let confirmed_component = entity_ref.get::<Confirmed>().unwrap();
    let Some(predicted) = confirmed_component.predicted else {
        return;
    };
    let components: Vec<ComponentId> = entity_ref
        .archetype()
        .components()
        .filter(|id| {
            component_registry
                .get_prediction_mode(*id)
                .is_ok_and(|mode| mode != ComponentSyncMode::None)
        })
        .collect();
    if components.is_empty() {
        return;
    }
    events.send(PredictedSyncEvent {
        confirmed,
        predicted,
        components,
    });
}

/// Sync any components that were added to the Confirmed entity onto the Predicted entity
/// and potentially add a PredictedHistory component
///
/// We use a global observer which will listen to the Insertion of **any** component on any Confirmed entity.
/// (using observers to react on insertion is more efficient than using the `Added` filter which iterates
/// through all confirmed archetypes)
///
/// We have some ordering constraints related to syncing hierarchy so we don't want to sync components
/// immediately here (because the ParentSync component might not be able to get mapped properly since the parent entity
/// might not be predicted yet). Therefore we send a PredictedSyncEvent so that all components can be synced at once.
fn added_on_confirmed_sync(
    // NOTE: we use OnInsert and not OnAdd because the confirmed entity might already have the component (for example if the client transferred authority to server)
    trigger: Trigger<OnInsert>,
    component_registry: Res<ComponentRegistry>,
    confirmed_query: Query<&Confirmed>,
    events: Option<ResMut<Events<PredictedSyncEvent>>>,
) {
    // `events` is None while we are inside the `apply_predicted_sync` system
    // that shouldn't be an issue because the components are being inserted only on Predicted entities
    // so we don't want to react to them
    let Some(mut events) = events else { return };
    // make sure the components were added on the confirmed entity
    let Ok(confirmed_component) = confirmed_query.get(trigger.target()) else {
        return;
    };
    let Some(predicted) = confirmed_component.predicted else {
        return;
    };
    let confirmed = trigger.target();

    // TODO: how do we avoid this allocation?

    // TODO: there is a bug where trigger.components() returns all components that were inserted, not just
    //  those that are currently watched by the observer!
    //  so we need to again filter components to only keep those that are predicted!
    let components: Vec<ComponentId> = trigger
        .components()
        .iter()
        .filter(|id| {
            component_registry
                .get_prediction_mode(**id)
                .is_ok_and(|mode| mode != ComponentSyncMode::None)
        })
        .copied()
        .collect();

    events.send(PredictedSyncEvent {
        confirmed,
        predicted,
        components,
    });
}

pub(crate) fn add_sync_systems(app: &mut App) {
    // we don't need to automatically update the events because they will be drained every frame
    app.init_resource::<Events<PredictedSyncEvent>>();

    let component_registry = app.world().resource::<ComponentRegistry>();

    // Sync components that are added on the Confirmed entity
    let mut observer = Observer::new(added_on_confirmed_sync);
    for component in component_registry.predicted_component_ids() {
        observer = observer.with_component(component);
    }
    app.world_mut().spawn(observer);

    // Sync components when the Confirmed component is added
    app.add_observer(confirmed_added_sync);

    // Apply the sync events
    app.add_systems(PreUpdate, apply_predicted_sync.in_set(PredictionSet::Sync));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client::RollbackState;
    use crate::prelude::{HistoryState, ShouldBePredicted, Tick};
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::ecs::system::RunSystemOnce;

    /// Test that components are synced from Confirmed to Predicted and that PredictionHistory is
    /// added correctly
    ///
    /// 1. Sync ComponentSyncMode::Full added to the confirmed entity + history is added
    /// 2. Add the history for ComponentSyncMode::Full that was added to the predicted entity
    /// 3. Sync ComponentSyncMode::Once added to the confirmed entity but don't add history
    /// 4. Sync ComponentSyncMode::Simple added to the confirmed entity but don't add history
    /// 5. For components that have MapEntities, the component gets mapped from Confirmed to Predicted
    /// 6. Sync pre-existing components when Confirmed is added to an entity
    #[test]
    fn test_confirmed_to_predicted_sync() {
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
        let tick = stepper.client_tick();
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
        stepper
            .client_app
            .world_mut()
            .entity_mut(predicted)
            .insert(ComponentCorrection(2.0));
        stepper.frame_step();
        let tick = stepper.client_tick();
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted)
                .get_mut::<PredictionHistory<ComponentCorrection>>()
                .expect("Expected prediction history to be added")
                .pop_until_tick(tick),
            Some(HistoryState::Updated(ComponentCorrection(2.0))),
            "Expected component value to be added to prediction history"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted)
                .get::<ComponentCorrection>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentCorrection(2.0),
            "Expected component to be added to predicted entity"
        );

        // 3. Don't add the history for ComponentSyncMode::Simple
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
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed)
            .insert(ComponentSyncModeOnce(1.0));
        stepper.frame_step();
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted)
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

        // 6. Sync components that were present on the confirmed entity before Confirmed is added
        let confirmed_2 = stepper
            .client_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
                ComponentSyncModeOnce(1.0),
                ComponentMapEntities(confirmed),
            ))
            .id();
        let predicted_2 = stepper
            .client_app
            .world_mut()
            .spawn(Predicted {
                confirmed_entity: Some(confirmed_2),
            })
            .id();
        stepper
            .client_app
            .world_mut()
            .entity_mut(confirmed_2)
            .insert(Confirmed {
                tick,
                predicted: Some(predicted_2),
                interpolated: None,
            });

        stepper.frame_step();
        let tick = stepper.client_tick();

        // check that the components were synced
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(predicted_2)
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
                .entity(predicted_2)
                .get::<ComponentSyncModeFull>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeFull(1.0),
            "Expected component to be added to predicted entity"
        );
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted_2)
                .get::<PredictionHistory<ComponentSyncModeOnce>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Once"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted_2)
                .get::<ComponentSyncModeOnce>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentSyncModeOnce(1.0),
            "Expected component to be added to predicted entity"
        );
        assert!(
            stepper
                .client_app
                .world()
                .entity(predicted_2)
                .get::<PredictionHistory<ComponentMapEntities>>()
                .is_none(),
            "Expected component value to not be added to prediction history for ComponentSyncMode::Simple"
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(predicted_2)
                .get::<ComponentMapEntities>()
                .expect("Expected component to be added to predicted entity"),
            &ComponentMapEntities(predicted),
            "Expected component to be added to predicted entity with entity mapping"
        );
    }

    // TODO: test that PredictionHistory is added when a component is added to a PrePredicted or PreSpawned entity

    /// Test that components are synced from Confirmed to Predicted simultaneously, not sequentially
    #[test]
    fn test_predicted_sync_batch() {
        let mut stepper = BevyStepper::default_no_init();
        // make sure that when ComponentSimple is added, ComponentOnce was also added
        stepper.client_app.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeSimple>,
             query: Query<(), With<ComponentSyncModeOnce>>| {
                assert!(query.get(trigger.target()).is_ok());
            },
        );
        // make sure that when ComponentOnce is added, ComponentSimple was also added
        // i.e. both components are added at the same time
        stepper.client_app.add_observer(
            |trigger: Trigger<OnAdd, ComponentSyncModeOnce>,
             query: Query<(), With<ComponentSyncModeSimple>>| {
                assert!(query.get(trigger.target()).is_ok());
            },
        );
        stepper.init();

        stepper.client_app.world_mut().spawn((
            ShouldBePredicted,
            ComponentSyncModeOnce(1.0),
            ComponentSyncModeSimple(1.0),
        ));
        stepper.frame_step();
        stepper.frame_step();

        // check that the components were synced to the predicted entity
        assert!(stepper
            .client_app
            .world_mut()
            .query_filtered::<(), (
                With<ComponentSyncModeOnce>,
                With<ComponentSyncModeSimple>,
                With<Predicted>
            )>()
            .single(stepper.client_app.world())
            .is_ok());
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
