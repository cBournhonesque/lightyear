#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::component::ComponentId;
use bevy::prelude::{Component, EntityWorldMut, TypePath};
use bevy::ptr::OwningPtr;
use core::alloc::Layout;
use core::ptr::NonNull;

/// An [`EntityWorldMut`] that buffers all insertions and removals so that they are all applied at once.
pub struct BufferedEntity<'w, 'b> {
    pub entity: EntityWorldMut<'w>,
    pub buffered: &'b mut BufferedChanges,
}

impl <'w, 'b> BufferedEntity<'w, 'b> {

    pub(crate) fn component_id<C: Component>(&mut self) -> ComponentId {
        // SAFETY: does not update the entity's location
        unsafe { self.entity.world_mut().register_component::<C>() }
    }

    pub(crate) fn apply(&mut self) {
        self.buffered.apply(&mut self.entity)
    }
}

#[derive(Debug, Default)]
pub struct BufferedChanges {
     insertions: TempWriteBuffer,
     removals: Vec<ComponentId>,
}


impl BufferedChanges {
    pub fn apply(&mut self, entity: &mut EntityWorldMut) {
        if !self.removals.is_empty() {
            entity.remove_by_ids(&self.removals);
        }
        self.removals.clear();

        if !self.insertions.is_empty() {
            // SAFETY: the `insertions` buffer is guaranteed to be valid for the lifetime of `self`.
            unsafe { self.insertions.batch_insert(entity); }
        }
    }

    /// # Safety
    /// The `component` must match the `component_id` type.
    pub unsafe fn insert<C: Component>(&mut self, component: C, component_id: ComponentId) {
        // SAFETY: the component C matches the `component_id`
        unsafe { self.insertions.buffer_insert_raw_ptrs(component, component_id) };
    }

    pub fn remove(&mut self, component_id: ComponentId) {
        self.removals.push(component_id);
    }
}

/// Temporary buffer to store component data that we want to insert
/// using `entity_world_mut.insert_by_ids`
#[derive(Debug, Default, PartialEq, TypePath)]
pub struct TempWriteBuffer {
    // temporary buffers to store the deserialized data to batch write
    // Raw storage where we can store the deserialized data bytes
    raw_bytes: Vec<u8>,
    // Positions of each component in the `raw_bytes` buffer
    component_ptrs_indices: Vec<usize>,
    // List of component ids
    pub(crate) component_ids: Vec<ComponentId>,
    // Position of the `component_ptr_indices` and `component_ids` list
    // This is needed because we can write into the buffer recursively.
    // For example if we write component A in the buffer, then call entity_mut_world.insert(A),
    // we might trigger an observer that inserts(B) in the buffer before it can be cleared
    cursor: usize,
}

impl TempWriteBuffer {
    pub(crate) fn is_empty(&self) -> bool {
        self.cursor == self.component_ids.len()
    }
    // TODO: also write a similar function for component removals, to handle recursive removals!

    /// Inserts the components that were buffered inside the EntityWorldMut
    ///
    /// # Safety
    /// `buffer_insert_raw_ptrs` must have been called beforehand
    pub unsafe fn batch_insert(&mut self, entity_world_mut: &mut EntityWorldMut) {
        if self.is_empty() {
            return;
        }
        // apply all commands from start_cursor to end
        // SAFETY: a value was insert in the cursor in a previous call to `buffer_insert_raw_ptrs`
        let start = self.cursor;
        // set the cursor position so that recursive calls only start reading the buffer from this
        // position
        self.cursor = self.component_ids.len();
        let start_index = self.component_ptrs_indices[start];
        // apply all buffer contents from `start` to the end
        unsafe {
            entity_world_mut.insert_by_ids(
                &self.component_ids[start..],
                self.component_ptrs_indices[start..].iter().map(|index| {
                    let ptr = NonNull::new_unchecked(self.raw_bytes.as_mut_ptr().add(*index));
                    OwningPtr::new(ptr)
                }),
            )
        };
        // clear the raw bytes that we inserted in the entity_world_mut
        self.component_ptrs_indices.drain(start..);
        self.component_ids.drain(start..);
        self.raw_bytes.drain(start_index..);
        self.cursor = start;
    }

    /// Store the component's raw bytes into a temporary buffer so that we can get an OwningPtr to it
    /// This function is called for all components that will be added to an entity, so that we can
    /// insert them all at once using `entity_world_mut.insert_by_ids`
    ///
    /// # Safety
    /// - the component C must match the `component_id `
    pub unsafe fn buffer_insert_raw_ptrs<C: Component>(
        &mut self,
        mut component: C,
        component_id: ComponentId,
    ) {
        let layout = Layout::new::<C>();
        // SAFETY: we are creating a pointer to the component data, which is non-null
        let ptr = unsafe { NonNull::new_unchecked(&mut component).cast::<u8>() };
        // make sure the Drop trait is not called when the `component` variable goes out of scope
        core::mem::forget(component);
        let count = layout.size();
        self.raw_bytes.reserve(count);
        let space =
            unsafe { NonNull::new_unchecked(self.raw_bytes.spare_capacity_mut()).cast::<u8>() };
        unsafe { space.copy_from_nonoverlapping(ptr, count) };
        let length = self.raw_bytes.len();
        // SAFETY: we are using the spare capacity of the Vec, so we know that the length is correct
        unsafe { self.raw_bytes.set_len(length + count) };
        self.component_ptrs_indices.push(length);
        self.component_ids.push(component_id);
    }
}