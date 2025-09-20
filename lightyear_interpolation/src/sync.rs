use crate::InterpolationMode;
use crate::plugin::InterpolationSet;
use crate::prelude::InterpolationRegistry;
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PreUpdate};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::change_detection::{Mut, Res, ResMut};
use bevy_ecs::component::{Component, ComponentId};
use bevy_ecs::entity::Entity;
use bevy_ecs::event::{Event, Events};
use bevy_ecs::observer::{Observer, Trigger};
use bevy_ecs::prelude::{EntityRef, IntoScheduleConfigs, Insert, Query, World};
use bevy_reflect::Reflect;
use lightyear_replication::components::{Confirmed, Replicated};
use lightyear_replication::prelude::ComponentRegistry;
use lightyear_replication::registry::buffered::BufferedChanges;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Plugin that syncs components that were inserted on the Confirmed entity to the Interpolated entity
pub(crate) struct SyncPlugin;

impl Plugin for SyncPlugin {
    fn build(&self, app: &mut App) {}

    fn cleanup(&self, app: &mut App) {
        // we don't need to automatically update the events because they will be drained every frame
        app.init_resource::<Events<InterpolatedSyncEvent>>();

        let interpolation_registry = app.world().resource::<InterpolationRegistry>();
        let component_registry = app.world().resource::<ComponentRegistry>();

        // Sync components that are added on the Confirmed entity
        let mut observer = Observer::new(added_on_confirmed_sync);
        for component_id in interpolation_registry
            .interpolation_map
            .keys()
            .filter_map(|k| {
                component_registry
                    .component_metadata_map
                    .get(k)
                    .map(|m| m.component_id)
            })
        {
            observer = observer.with_component(component_id);
        }
        app.world_mut().spawn(observer);

        // Sync components when the Confirmed component is added
        app.add_observer(confirmed_added_sync);

        // Apply the sync events
        app.configure_sets(PreUpdate, InterpolationSet::Sync);
        app.add_systems(
            PreUpdate,
            apply_interpolated_sync.in_set(InterpolationSet::Sync),
        );
    }
}

#[derive(Event, Debug)]
struct InterpolatedSyncEvent {
    confirmed: Entity,
    interpolated: Entity,
    manager: Entity,
    components: Vec<ComponentId>,
}

/// Buffer the stores components that we need to sync from the Confirmed to the Interpolated entity
#[derive(Component, Default, Deref, DerefMut, Reflect)]
pub(crate) struct InterpolationSyncBuffer(BufferedChanges);

/// Sync components from confirmed entity to interpolated entity
// TODO: sync removals! when C gets removed on confirmed, we should remove C + ConfirmedHistory<C> on interpolated
fn apply_interpolated_sync(world: &mut World) {
    world.resource_scope(|world, mut events: Mut<Events<InterpolatedSyncEvent>>| {
        events.drain().for_each(|event| {
            // NOTE: we cannot use `world.resource_scope::<ComponentRegistry>` because doing the sync
            //  might trigger other Observers that might also use the ComponentRegistry
            //  Instead we'll use UnsafeWorldCell since the rest of the world does not modify the registry
            let unsafe_world = world.as_unsafe_world_cell();
            let interpolated_registry =
                unsafe { unsafe_world.get_resource::<InterpolationRegistry>() }.unwrap();
            let component_registry =
                unsafe { unsafe_world.get_resource::<ComponentRegistry>() }.unwrap();
            let buffer = &mut unsafe {
                unsafe_world
                    .world_mut()
                    .get_mut::<InterpolationSyncBuffer>(event.manager)
            }
            .unwrap();
            trace!(
                "Sync from confirmed {:?} to interpolated {:?}",
                event.confirmed, event.interpolated
            );

            let world = unsafe { unsafe_world.world_mut() };

            // sync all components from the predicted to the confirmed entity and possibly add the PredictedHistory
            interpolated_registry.batch_sync(
                component_registry,
                &event.components,
                event.confirmed,
                event.interpolated,
                event.manager,
                world,
                buffer,
            );
        })
    });
}

/// When the Confirmed component is added, sync components to the Interpolated entity
///
/// This is needed in two cases:
/// - when an entity is replicated, the components are replicated onto the Confirmed entity before the Confirmed
///   component is added
/// - when a client spawned on the client transfers authority to the server, the Confirmed
///   component can be added even though the entity already had existing components
///
/// We have some ordering constraints related to syncing hierarchy so we don't want to sync components
/// immediately here (because the ParentSync component might not be able to get mapped properly since the parent entity
/// might not be interpolated yet). Therefore we send a InterpolatedSyncEvent so that all components can be synced at once.
fn confirmed_added_sync(
    trigger: On<Insert, Confirmed>,
    confirmed_query: Query<EntityRef>,
    interpolation_registry: Res<InterpolationRegistry>,
    component_registry: Res<ComponentRegistry>,
    events: Option<ResMut<Events<InterpolatedSyncEvent>>>,
) {
    // `events` is None while we are inside the `apply_predicted_sync` system
    // that shouldn't be an issue because the components are being inserted only on Predicted entities
    // so we don't want to react to them
    let Some(mut events) = events else { return };
    let confirmed = trigger.entity;
    let entity_ref = confirmed_query.get(confirmed).unwrap();
    let confirmed_component = entity_ref.get::<Confirmed>().unwrap();
    let Some(interpolated) = confirmed_component.interpolated else {
        return;
    };
    let components: Vec<ComponentId> = entity_ref
        .archetype()
        .components()
        .filter(|id| {
            interpolation_registry
                .get_interpolation_mode(*id, &component_registry)
                .is_ok_and(|mode| mode != InterpolationMode::None)
        })
        .collect();
    if components.is_empty() {
        return;
    }
    let replicated = entity_ref.get::<Replicated>().unwrap();
    events.send(InterpolatedSyncEvent {
        confirmed,
        interpolated,
        manager: replicated.receiver,
        components,
    });
}

/// Sync any components that were added to the Confirmed entity onto the Interpolated entity
/// and potentially add a InterpolatedHistory component
///
/// We use a global observer which will listen to the Insertion of **any** interpolated component on any Confirmed entity.
/// (using observers to react on insertion is more efficient than using the `Added` filter which iterates
/// through all confirmed archetypes)
///
/// We have some ordering constraints related to syncing hierarchy so we don't want to sync components
/// immediately here (because the ParentSync component might not be able to get mapped properly since the parent entity
/// might not be interpolated yet). Therefore we send a InterpolatedSyncEvent so that all components can be synced at once.
fn added_on_confirmed_sync(
    // NOTE: we use Insert and not Add because the confirmed entity might already have the component (for example if the client transferred authority to server)
    trigger: On<Insert>,
    interpolation_registry: Res<InterpolationRegistry>,
    component_registry: Res<ComponentRegistry>,
    confirmed_query: Query<(&Confirmed, &Replicated)>,
    events: Option<ResMut<Events<InterpolatedSyncEvent>>>,
) {
    // `events` is None while we are inside the `apply_interpolated_sync` system
    // that shouldn't be an issue because the components are being inserted only on Interpolated entities
    // so we don't want to react to them
    let Some(mut events) = events else { return };
    // make sure the components were added on the confirmed entity
    let Ok((confirmed_component, replicated)) = confirmed_query.get(trigger.entity) else {
        return;
    };
    let Some(interpolated) = confirmed_component.interpolated else {
        return;
    };
    let confirmed = trigger.entity;

    // TODO: how do we avoid this allocation?

    // TODO: there is a bug where trigger.components() returns all components that were inserted, not just
    //  those that are currently watched by the observer!
    //  so we need to again filter components to only keep those that are predicted!
    let components: Vec<ComponentId> = trigger
        .components()
        .iter()
        .filter(|id| {
            interpolation_registry
                .get_interpolation_mode(**id, &component_registry)
                .is_ok_and(|mode| mode != InterpolationMode::None)
        })
        .copied()
        .collect();

    events.send(InterpolatedSyncEvent {
        confirmed,
        interpolated,
        manager: replicated.receiver,
        components,
    });
}
