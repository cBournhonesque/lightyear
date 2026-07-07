//! Low-level ECS access helpers shared by Lightyear crates.

use bevy_ecs::{
    archetype::Archetype,
    component::{Component, ComponentId, StorageType},
    storage::Table,
    world::unsafe_world_cell::UnsafeWorldCell,
};
use core::cell::UnsafeCell;

/// Component column state for table-optimized archetype scans.
#[derive(Clone, Copy)]
pub enum ComponentTableColumn<'w, C> {
    /// The component is present and table-stored on this archetype.
    Table(&'w [UnsafeCell<C>]),
    /// The component type is not registered or not present on this archetype.
    Missing,
    /// The component is present but not table-stored.
    NonTable,
}

/// Returns the table backing `archetype`.
pub fn table_for_archetype<'w>(
    world: UnsafeWorldCell<'w>,
    archetype: &Archetype,
) -> Option<&'w Table> {
    // SAFETY: this only returns the table for the provided archetype id. The
    // caller is still responsible for respecting declared system access when
    // reading or writing columns from the returned table.
    unsafe { world.storages().tables.get(archetype.table_id()) }
}

/// Returns a typed table column for `component_id`.
pub fn table_component_slice<C: Component>(
    table: &Table,
    component_id: ComponentId,
) -> Option<&[UnsafeCell<C>]> {
    // SAFETY: callers pass component ids registered for `C`. This helper is
    // used by type-erased systems after their cache has resolved concrete ids.
    unsafe { table.get_data_slice_for::<C>(component_id) }
}

/// Returns a typed table column only when the cached storage is table-backed.
pub fn table_component_slice_if_table<C: Component>(
    table: &Table,
    component_id: ComponentId,
    storage: Option<StorageType>,
) -> Option<&[UnsafeCell<C>]> {
    match storage {
        Some(StorageType::Table) => table_component_slice::<C>(table, component_id),
        _ => None,
    }
}

/// Returns table-column state for component `C` on `archetype`.
pub fn component_table_column<'w, C: Component>(
    world: UnsafeWorldCell<'w>,
    archetype: &Archetype,
    table: &'w Table,
) -> ComponentTableColumn<'w, C> {
    let Some(component_id) = world.components().component_id::<C>() else {
        return ComponentTableColumn::Missing;
    };
    if !archetype.contains(component_id) {
        return ComponentTableColumn::Missing;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
        return ComponentTableColumn::NonTable;
    };
    table_component_slice::<C>(table, component_id)
        .map_or(ComponentTableColumn::NonTable, ComponentTableColumn::Table)
}
