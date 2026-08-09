//! Helpers for low-level ECS operations.

use bevy_ecs::archetype::ArchetypeEntity;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::ptr::{Ptr, PtrMut};
use bevy_ecs::query::{IterQueryData, QueryFilter, QueryItem};
use bevy_ecs::storage::TableId;
use bevy_ecs::system::Query;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;

/// Iterates a mutable query serially when it contains at most a small number of items, and in
/// parallel otherwise.
///
/// The default serial threshold is one item:
///
/// ```ignore
/// adaptive_for_each_mut!(query, |item| process(item));
/// ```
///
/// An optional threshold keeps queries with at most that many items serial. Parallel iteration is
/// used only when the query contains more items than the threshold:
///
/// ```ignore
/// adaptive_for_each_mut!(query, 4, |item| process(item));
/// ```
///
/// The adapter inspects only enough read-only query items to choose a mode, then performs the
/// mutable iteration.
#[macro_export]
macro_rules! adaptive_for_each_mut {
    ($query:expr, |$item:pat_param| $body:expr $(,)?) => {
        $crate::ecs::AdaptiveQueryIterMut::new(&mut $query, 1).for_each(|$item| $body)
    };
    ($query:expr, $serial_threshold:expr, |$item:pat_param| $body:expr $(,)?) => {
        $crate::ecs::AdaptiveQueryIterMut::new(&mut $query, $serial_threshold)
            .for_each(|$item| $body)
    };
    // Adapter form for composing with an existing `.for_each` call.
    ($query:expr $(,)?) => {
        $crate::ecs::AdaptiveQueryIterMut::new(&mut $query, 1)
    };
}

/// Mutable query adapter returned by [`adaptive_for_each_mut!`].
///
/// Call [`for_each`](Self::for_each) to select serial or parallel iteration based on the number of
/// matching query items.
#[doc(hidden)]
pub struct AdaptiveQueryIterMut<'query, 'world, 'state, D, F>
where
    D: IterQueryData,
    F: QueryFilter,
{
    query: &'query mut Query<'world, 'state, D, F>,
    serial_threshold: usize,
}

impl<'query, 'world, 'state, D, F> AdaptiveQueryIterMut<'query, 'world, 'state, D, F>
where
    D: IterQueryData,
    F: QueryFilter,
{
    #[doc(hidden)]
    pub fn new(query: &'query mut Query<'world, 'state, D, F>, serial_threshold: usize) -> Self {
        Self {
            query,
            serial_threshold,
        }
    }

    /// Applies `func` to every matching item.
    pub fn for_each<Func>(self, func: Func)
    where
        Func: for<'item> Fn(QueryItem<'item, 'state, D>) + Send + Sync + Clone,
    {
        if self.query.iter().nth(self.serial_threshold).is_some() {
            self.query.par_iter_mut().for_each(func);
        } else {
            self.query.iter_mut().for_each(func);
        }
    }
}

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

#[cfg(test)]
mod tests {
    use bevy_app::{App, TaskPoolPlugin};
    use bevy_ecs::prelude::*;

    #[derive(Component)]
    struct Value(u32);

    #[test]
    fn adaptive_iteration_supports_default_and_custom_thresholds() {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        let world = app.world_mut();
        let first = world.spawn(Value(0)).id();
        let mut query_state = world.query::<&mut Value>();

        {
            let mut query = query_state.query_mut(world);
            crate::adaptive_for_each_mut!(query, |mut value| value.0 += 1);
        }

        let second = world.spawn(Value(0)).id();
        {
            let mut query = query_state.query_mut(world);
            crate::adaptive_for_each_mut!(query, 2, |mut value| value.0 += 1);
        }
        {
            let mut query = query_state.query_mut(world);
            crate::adaptive_for_each_mut!(query, |mut value| value.0 += 1);
        }

        assert_eq!(world.get::<Value>(first).unwrap().0, 3);
        assert_eq!(world.get::<Value>(second).unwrap().0, 2);
    }
}
