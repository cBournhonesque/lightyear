//! Helpers for low-level ECS operations.

use bevy_ecs::archetype::ArchetypeEntity;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::ptr::{Ptr, PtrMut};
use bevy_ecs::storage::TableId;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;

/// Extracts a component as [`Ptr`] and its ticks from a table or sparse set, depending on its storage type.
///
/// # Safety
///
/// The component must be present in this archetype, have the specified storage type and we must have write access to it.
pub unsafe fn get_component_unchecked_mut<'w>(
    unsafe_world_cell: UnsafeWorldCell<'w>,
    entity: &'w ArchetypeEntity,
    table_id: TableId,
    storage: StorageType,
    component_id: ComponentId,
) -> PtrMut<'w> {
    let storages = unsafe { unsafe_world_cell.storages() };
    match storage {
        // SAFETY: we know from the accesses that we have unique write access to these components
        StorageType::Table => unsafe {
            let table = storages.tables.get(table_id).unwrap_unchecked();
            table
                .get_component(component_id, entity.table_row())
                .unwrap_unchecked()
                .assert_unique()
        },
        StorageType::SparseSet => unsafe {
            let sparse_set = storages.sparse_sets.get(component_id).unwrap_unchecked();
            sparse_set
                .get(entity.id())
                .unwrap_unchecked()
                .assert_unique()
        },
    }
}

/// Extracts a component as [`Ptr`] and its ticks from a table or sparse set, depending on its storage type.
///
/// # Safety
///
/// The component must be present in this archetype, have the specified storage type and we must have read access to it.
pub unsafe fn get_component_unchecked<'w>(
    unsafe_world_cell: UnsafeWorldCell<'w>,
    entity: &'w ArchetypeEntity,
    table_id: TableId,
    storage: StorageType,
    component_id: ComponentId,
) -> Ptr<'w> {
    let storages = unsafe { unsafe_world_cell.storages() };
    match storage {
        // SAFETY: we know from the accesses that we have unique write access to these components
        StorageType::Table => unsafe {
            let table = storages.tables.get(table_id).unwrap_unchecked();
            table
                .get_component(component_id, entity.table_row())
                .unwrap_unchecked()
        },
        StorageType::SparseSet => unsafe {
            let sparse_set = storages.sparse_sets.get(component_id).unwrap_unchecked();
            sparse_set.get(entity.id()).unwrap_unchecked()
        },
    }
}
