//! Managed the history buffer, which is a buffer of the past predicted component states,
//! so that whenever we receive an update from the server we can compare the predicted entity's history with the server update.
use crate::correction::Correction;
use crate::manager::{PredictionManager, PredictionResource};
use crate::plugin::{PredictionFilter, PredictionSet};
use crate::prespawn::PreSpawned;
use crate::registry::PredictionRegistry;
use crate::{Predicted, PredictionMode, SyncComponent};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::app::App;
use bevy::ecs::component::ComponentId;
use bevy::prelude::*;
use core::fmt::Debug;
use core::ops::Deref;
use lightyear_core::history_buffer::HistoryBuffer;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::timeline::SyncEvent;
use lightyear_replication::components::PrePredicted;
use lightyear_replication::prelude::{Confirmed, ReplicationSet};
use lightyear_replication::registry::registry::ComponentRegistry;
use lightyear_sync::prelude::InputTimeline;

pub type PredictionHistory<C> = HistoryBuffer<C>;

/// If PredictionMode::Full, we store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + Clone>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();

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
    trigger: Trigger<SyncEvent<InputTimeline>>,
    mut query: Query<(&mut PredictionHistory<C>, Option<&mut Correction<C>>)>,
) {
    for (mut history, correction) in query.iter_mut() {
        history.update_ticks(trigger.tick_delta);
        if let Some(mut correction) = correction {
            correction.update_ticks(trigger.tick_delta);
        }
    }
}

/// If a component is removed on the Predicted entity, and the PredictionMode == FULL
/// Add the removal to the history (for potential rollbacks)
pub(crate) fn apply_component_removal_predicted<C: Component>(
    trigger: Trigger<OnRemove, C>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
    timeline: Single<&LocalTimeline, PredictionFilter>,
) {
    let tick = timeline.tick();
    // if the component was removed from the Predicted entity, add the Removal to the history
    if let Ok(mut history) = predicted_query.get_mut(trigger.target()) {
        // tick for which we will record the history (either the current client tick or the current rollback tick)
        history.add_remove(tick);
    }
}

/// If the component was removed from the Confirmed entity:
/// - if the PredictionMode == ONCE, do nothing (we only care about replicating the component once)
/// - if the PredictionMode == SIMPLE, remove the component from the Predicted entity
/// - if the PredictionMode == FULL, do nothing. We might get a rollback by comparing with the history.
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

/// If PredictionMode == Simple, when we receive a server update we want to apply it to the predicted entity
#[allow(clippy::type_complexity)]
pub(crate) fn apply_confirmed_update<C: SyncComponent>(
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    manager: Single<&PredictionManager>,
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
                        prediction_registry.prediction_mode::<C>(),
                        PredictionMode::Simple
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
            let prediction_registry =
                unsafe { unsafe_world.get_resource::<PredictionRegistry>() }.unwrap();
            let component_registry =
                unsafe { unsafe_world.get_resource::<ComponentRegistry>() }.unwrap();
            let link_entity = unsafe {
                unsafe_world
                    .get_resource::<PredictionResource>()
                    .unwrap()
                    .link_entity
            };
            let buffer = &mut unsafe {
                unsafe_world
                    .world_mut()
                    .get_mut::<PredictionManager>(link_entity)
            }
            .unwrap()
            .buffer;
            trace!(
                "Sync from confirmed {:?} to predicted {:?}",
                event.confirmed, event.predicted
            );

            let world = unsafe { unsafe_world.world_mut() };

            // sync all components from the predicted to the confirmed entity and possibly add the PredictedHistory
            prediction_registry.batch_sync(
                component_registry,
                &event.components,
                event.confirmed,
                event.predicted,
                world,
                buffer,
            );
        })
    });
}

/// If a PredictionMode::Full gets added to [`PrePredicted`] or [`PreSpawned`] entity,
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
    prediction_registry: Res<PredictionRegistry>,
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
            prediction_registry
                .get_prediction_mode(*id, &component_registry)
                .is_ok_and(|mode| mode != PredictionMode::None)
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
/// We use a global observer which will listen to the Insertion of **any** predicted component on any Confirmed entity.
/// (using observers to react on insertion is more efficient than using the `Added` filter which iterates
/// through all confirmed archetypes)
///
/// We have some ordering constraints related to syncing hierarchy so we don't want to sync components
/// immediately here (because the ParentSync component might not be able to get mapped properly since the parent entity
/// might not be predicted yet). Therefore we send a PredictedSyncEvent so that all components can be synced at once.
fn added_on_confirmed_sync(
    // NOTE: we use OnInsert and not OnAdd because the confirmed entity might already have the component (for example if the client transferred authority to server)
    trigger: Trigger<OnInsert>,
    prediction_registry: Res<PredictionRegistry>,
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
            prediction_registry
                .get_prediction_mode(**id, &component_registry)
                .is_ok_and(|mode| mode != PredictionMode::None)
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

    let prediction_registry = app.world().resource::<PredictionRegistry>();
    let component_registry = app.world().resource::<ComponentRegistry>();

    // Sync components that are added on the Confirmed entity
    let mut observer = Observer::new(added_on_confirmed_sync);
    for component in prediction_registry
        .prediction_map
        .keys()
        .map(|k| component_registry.kind_to_component_id[k])
    {
        observer = observer.with_component(component);
    }
    app.world_mut().spawn(observer);

    // Sync components when the Confirmed component is added
    app.add_observer(confirmed_added_sync);

    // Apply the sync events
    // make sure to Sync before the RelationshipSync systems run
    app.configure_sets(
        PreUpdate,
        PredictionSet::Sync.before(ReplicationSet::ReceiveRelationships),
    );
    app.add_systems(PreUpdate, apply_predicted_sync.in_set(PredictionSet::Sync));
}
