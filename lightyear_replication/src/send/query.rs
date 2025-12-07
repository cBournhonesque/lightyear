//! Taken from replicon.

use crate::prelude::*;
use bevy_ecs::component::{ComponentId, ComponentTicks, StorageType, Tick};
use bevy_ecs::prelude::*;
use bevy_ecs::query::{FilteredAccess, FilteredAccessSet};
use bevy_ecs::storage::{TableId, TableRow};
use bevy_ecs::system::{ReadOnlySystemParam, SystemMeta, SystemParam};
use bevy_ecs::world::unsafe_world_cell::{UnsafeEntityCell, UnsafeWorldCell};
use bevy_ptr::Ptr;

/// Like [`Query`], but provides dynamic access only for replicated components.
///
/// We don't use [`FilteredEntityRef`](bevy::ecs::world::FilteredEntityRef) to avoid access checks
/// and [`StorageType`] fetch (we cache this information on replicated archetypes).
pub(crate) struct ReplicationQuery<'w, 's> {
    world: UnsafeWorldCell<'w>,
    state: &'s ReplicationQueryState,
}

impl<'w> ReplicationQuery<'w, '_> {
    /// Extracts a component as [`Ptr`] and its ticks from a table or sparse set, depending on its storage type.
    ///
    /// # Safety
    ///
    /// The component must be present in this archetype, have the specified storage type, and be previously marked for replication.
    pub(super) unsafe fn get_component_unchecked(
        &self,
        entity: Entity,
        table_row: TableRow,
        table_id: TableId,
        storage: StorageType,
        component_id: ComponentId,
    ) -> (Ptr<'w>, ComponentTicks) {
        debug_assert!(
            self.state
                .component_access
                .access()
                .has_component_read(component_id)
        );

        // SAFETY: caller ensured the component is replicated.
        let storages = unsafe { self.world.storages() };
        match storage {
            StorageType::Table => unsafe {
                let table = storages.tables.get(table_id).unwrap_unchecked();
                // TODO: re-use column lookup, asked in https://github.com/bevyengine/bevy/issues/16593.
                let component: Ptr<'w> = table
                    .get_component(component_id, table_row)
                    .unwrap_unchecked();
                let ticks = table
                    .get_ticks_unchecked(component_id, table_row)
                    .unwrap_unchecked();

                (component, ticks)
            },
            StorageType::SparseSet => unsafe {
                let sparse_set = storages.sparse_sets.get(component_id).unwrap_unchecked();
                let component = sparse_set.get(entity).unwrap_unchecked();
                let ticks = sparse_set.get_ticks(entity).unwrap_unchecked();

                (component, ticks)
            },
        }
    }

    pub(super) fn cell(&self, entity: Entity) -> UnsafeEntityCell {
        self.world.get_entity(entity).unwrap()
    }
}

unsafe impl SystemParam for ReplicationQuery<'_, '_> {
    type State = ReplicationQueryState;
    type Item<'w, 's> = ReplicationQuery<'w, 's>;

    fn init_state(world: &mut World) -> Self::State {
        let mut component_access = FilteredAccess::default();

        component_access.add_component_read(world.register_component::<Replicate>());
        component_access.add_component_read(world.register_component::<ReplicateLikeChildren>());
        component_access.add_component_write(world.register_component::<ReplicationState>());
        component_access.add_component_read(world.register_component::<NetworkVisibility>());
        component_access.add_component_read(world.register_component::<ReplicateLike>());
        component_access.add_component_read(world.register_component::<ControlledBy>());
        component_access.add_component_read(world.register_component::<PreSpawned>());

        world
            .resource::<ComponentRegistry>()
            .component_metadata_map
            .iter()
            .for_each(|(kind, m)| {
                component_access.add_component_read(m.component_id);
                if let Some(r) = &m.replication {
                    component_access.add_component_read(r.overrides_component_id);
                }
            });

        Self::State { component_access }
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        _world: &mut World,
    ) {
        let conflicts = component_access_set.get_conflicts_single(&state.component_access);
        if !conflicts.is_empty() {
            panic!(
                "replicated components in system `{}` shouldn't be in conflict with other system parameters",
                system_meta.name(),
            );
        }

        component_access_set.add(state.component_access.clone());
    }

    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        _system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        _change_tick: Tick,
    ) -> Self::Item<'world, 'state> {
        ReplicationQuery { world, state }
    }
}

unsafe impl ReadOnlySystemParam for ReplicationQuery<'_, '_> {}

pub(crate) struct ReplicationQueryState {
    /// All replicated components.
    ///
    /// Used only in debug to check component access.
    component_access: FilteredAccess,
}
