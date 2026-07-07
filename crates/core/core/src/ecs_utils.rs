//! Low-level ECS access helpers shared by Lightyear crates.

use bevy_ecs::{
    archetype::Archetype,
    component::{Component, ComponentId, Mutable, StorageType},
    storage::Table,
    world::unsafe_world_cell::UnsafeWorldCell,
};
use core::cell::UnsafeCell;

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

/// Replaces an entity's component value through Bevy's change-detection-aware
/// mutable access.
///
/// Type-erased systems often read histories directly from table columns for
/// efficient archetype scans. Writing a live component through the same raw
/// column access would update its value without updating its change tick. This
/// helper preserves normal `Mut<C>` semantics while keeping the surrounding
/// scan type-erased.
///
/// Returns `false` when the entity no longer exists or does not contain `C`.
///
/// # Safety
///
/// The caller must have exclusive access to `C` and must not hold any other
/// reference to this entity's `C`.
pub unsafe fn write_component_with_change_detection<C: Component<Mutability = Mutable>>(
    world: UnsafeWorldCell,
    entity: bevy_ecs::entity::Entity,
    value: C,
) -> bool {
    let Ok(entity) = world.get_entity(entity) else {
        return false;
    };
    // SAFETY: upheld by the caller. `get_mut` returns Bevy's `Mut<C>`, so
    // assigning through it updates the component's change tick and caller.
    let Some(mut component) = (unsafe { entity.get_mut::<C>() }) else {
        return false;
    };
    *component = value;
    true
}
