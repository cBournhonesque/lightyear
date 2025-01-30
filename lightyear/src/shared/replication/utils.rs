use bevy::ecs::component::{ComponentId, StorageType, Tick, TickCells};
use bevy::ecs::entity::EntityLocation;
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::prelude::*;
#[allow(unused_imports)]
use bevy::ptr::{Ptr, UnsafeCellDeref};
use std::any::TypeId;
#[cfg(feature = "track_location")]
use std::{cell::UnsafeCell, panic::Location};

#[cfg(feature = "track_location")]
pub(crate) type MaybeUnsafeCellLocation<'a> = &'a UnsafeCell<&'static Location<'static>>;

#[cfg(not(feature = "track_location"))]
pub(crate) type MaybeUnsafeCellLocation<'a> = ();

pub(crate) fn get_ref<C: Component>(
    world: &World,
    entity: Entity,
    last_run: Tick,
    this_run: Tick,
) -> Ref<C> {
    unsafe {
        let component_id = world
            .components()
            .get_id(TypeId::of::<C>())
            .unwrap_unchecked();
        let world = world.as_unsafe_world_cell_readonly();
        let entity_cell = world.get_entity(entity).unwrap_unchecked();
        get_component_and_ticks(
            world,
            component_id,
            C::STORAGE_TYPE,
            entity,
            entity_cell.location(),
        )
        .map(|(value, cells, _caller)| {
            Ref::new(
                value.deref::<C>(),
                cells.added.deref(),
                cells.changed.deref(),
                last_run,
                this_run,
                #[cfg(feature = "track_location")]
                _caller.deref(),
            )
        })
        .unwrap_unchecked()
    }
}

// Utility function to return
#[inline]
unsafe fn get_component_and_ticks(
    world: UnsafeWorldCell<'_>,
    component_id: ComponentId,
    storage_type: StorageType,
    entity: Entity,
    location: EntityLocation,
) -> Option<(Ptr<'_>, TickCells<'_>, MaybeUnsafeCellLocation<'_>)> {
    match storage_type {
        StorageType::Table => {
            let table = unsafe { world.storages().tables.get(location.table_id) }?;

            // SAFETY: archetypes only store valid table_rows and caller ensure aliasing rules
            Some((
                table.get_component(component_id, location.table_row)?,
                TickCells {
                    added: table
                        .get_added_tick(component_id, location.table_row)
                        .unwrap_unchecked(),
                    changed: table
                        .get_changed_tick(component_id, location.table_row)
                        .unwrap_unchecked(),
                },
                #[cfg(feature = "track_location")]
                table
                    .get_changed_by(component_id, location.table_row)
                    .unwrap_unchecked(),
                #[cfg(not(feature = "track_location"))]
                (),
            ))
        }
        StorageType::SparseSet => {
            let storage = unsafe { world.storages() }.sparse_sets.get(component_id)?;
            storage.get_with_ticks(entity)
        }
    }
}
