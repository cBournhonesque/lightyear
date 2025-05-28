use crate::components::ComponentReplicationConfig;
use crate::delta::{DeltaComponentHistory, DeltaMessage, DeltaType, Diffable};
use crate::registry::buffered::BufferedEntity;
use crate::registry::registry::ComponentRegistry;
use crate::registry::replication::ReplicationMetadata;
use crate::registry::{ComponentError, ComponentKind};
#[cfg(not(feature = "std"))]
use alloc::boxed::Box;
use bevy::ecs::component::{ComponentId, Mutable};
use bevy::prelude::{Component, World};
use bevy::ptr::{Ptr, PtrMut};
use core::any::TypeId;
use core::ptr::NonNull;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::ContextDeserializeFns;
use lightyear_serde::writer::Writer;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::trace;

impl ComponentRegistry {
    /// Register delta compression functions for a component
    pub fn set_delta_compression<C: Component<Mutability = Mutable> + PartialEq + Diffable>(
        &mut self,
        world: &mut World,
    ) where
        C::Delta: Serialize + DeserializeOwned,
    {
        let kind = ComponentKind::of::<C>();
        let delta_kind = ComponentKind::of::<DeltaMessage<C::Delta>>();
        // add the delta as a message
        self.register_component::<DeltaMessage<C::Delta>>(world);
        // add delta-related type-erased functions
        self.delta_fns_map.insert(kind, ErasedDeltaFns::new::<C>());
        // add write/remove functions associated with the delta component's net_id
        // (since the serialized message will contain the delta component's net_id)
        // update the write function to use the delta compression logic
        self.replication_map.insert(
            delta_kind,
            ReplicationMetadata::new(
                ComponentReplicationConfig::default(),
                ComponentId::new(0),
                buffer_insert_delta::<C>,
            )
        );

    }

    /// # Safety
    /// the Ptr must correspond to the correct ComponentKind
    pub unsafe fn erased_clone(
        &self,
        data: Ptr,
        kind: ComponentKind,
    ) -> Result<NonNull<u8>, ComponentError> {
        let delta_fns = self
            .delta_fns_map
            .get(&kind)
            .ok_or(ComponentError::MissingDeltaFns)?;
        Ok(unsafe { (delta_fns.clone)(data) })
    }

    /// # Safety
    /// the data from the Ptr must correspond to the correct ComponentKind
    pub unsafe fn erased_drop(
        &self,
        data: NonNull<u8>,
        kind: ComponentKind,
    ) -> Result<(), ComponentError> {
        let delta_fns = self
            .delta_fns_map
            .get(&kind)
            .ok_or(ComponentError::MissingDeltaFns)?;
        unsafe { (delta_fns.drop)(data) };
        Ok(())
    }

    /// # Safety
    /// The Ptrs must correspond to the correct ComponentKind
    pub unsafe fn serialize_diff(
        &self,
        start_tick: Tick,
        start: Ptr,
        new: Ptr,
        writer: &mut Writer,
        // kind for C, not for C::Delta
        kind: ComponentKind,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), ComponentError> {
        let delta_fns = self
            .delta_fns_map
            .get(&kind)
            .ok_or(ComponentError::MissingDeltaFns)?;

        let delta = unsafe { (delta_fns.diff)(start_tick, start, new) };
        self.erased_serialize(
            unsafe { Ptr::new(delta) },
            writer,
            delta_fns.delta_kind,
            entity_map,
        )?;
        // drop the delta message
        unsafe { (delta_fns.drop_delta_message)(delta) };
        Ok(())
    }

    /// # Safety
    /// The Ptrs must correspond to the correct ComponentKind
    pub unsafe fn serialize_diff_from_base_value(
        &self,
        component_data: Ptr,
        writer: &mut Writer,
        // kind for C, not for C::Delta
        kind: ComponentKind,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), ComponentError> {
        let delta_fns = self
            .delta_fns_map
            .get(&kind)
            .ok_or(ComponentError::MissingDeltaFns)?;
        let delta = unsafe { (delta_fns.diff_from_base)(component_data) };
        // SAFETY: the delta is a valid pointer to a DeltaMessage<C::Delta>
        self.erased_serialize(
            unsafe { Ptr::new(delta) },
            writer,
            delta_fns.delta_kind,
            entity_map,
        )?;
        // drop the delta message
        unsafe { (delta_fns.drop_delta_message)(delta) };
        Ok(())
    }
}

    /// Insert a component delta into the entity.
    /// If the component is not present on the entity, we put it in a temporary buffer
    /// so that all components can be inserted at once
    fn buffer_insert_delta<C: Component<Mutability = Mutable> + PartialEq + Diffable>(
        deserialize: ContextDeserializeFns<ReceiveEntityMap, DeltaMessage<C::Delta>, DeltaMessage<C::Delta>>,
        reader: &mut Reader,
        tick: Tick,
        entity_mut: &mut BufferedEntity,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), ComponentError> {
        let kind = ComponentKind::of::<C>();
        let component_id = entity_mut.component_id::<C>();
        trace!(
            ?kind,
            ?component_id,
            "Writing component delta {} to entity",
            core::any::type_name::<C>()
        );
        let delta = deserialize.deserialize(entity_map, reader)?;
        match delta.delta_type {
            DeltaType::Normal { previous_tick } => {
                unreachable!(
                    "buffer_insert_delta should only be called for FromBase deltas since the component is being inserted"
                );
            }
            DeltaType::FromBase => {
                let mut new_value = C::base_value();
                new_value.apply_diff(&delta.delta);
                // clone the value so that we can insert it in the history
                let cloned_value = new_value.clone();

                // if the component is on the entity, no need to insert
                if let Some(mut c) = entity_mut.entity.get_mut::<C>() {
                    // only apply the update if the component is different, to not trigger change detection
                    if c.as_ref() != &new_value {
                        *c = new_value;
                    }
                } else {
                    // use the component id of C, not DeltaMessage<C>
                    // SAFETY: we are inserting a component of type C, which matches the component_id
                    unsafe { entity_mut.buffered.insert::<C>(new_value, component_id); }
                }
                // store the component value in the delta component history, so that we can compute
                // diffs from it
                if let Some(mut history) = entity_mut.entity.get_mut::<DeltaComponentHistory<C>>() {
                    history.buffer.insert(tick, cloned_value);
                } else {
                    // create a DeltaComponentHistory and insert the value
                    let mut history = DeltaComponentHistory::default();
                    history.buffer.insert(tick, cloned_value);
                    entity_mut.entity.insert(history);
                }
            }
        }
        Ok(())
    }

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedDiffFn = unsafe fn(start_tick: Tick, start: Ptr, present: Ptr) -> NonNull<u8>;
type ErasedBaseDiffFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);
type ErasedDropFn = unsafe fn(data: NonNull<u8>);

/// SAFETY: the Ptr must be a valid pointer to a value of type C
unsafe fn erased_clone<C: Clone>(data: Ptr) -> NonNull<u8> {
    unsafe {
        let cloned: C = data.deref::<C>().clone();
        let leaked_data = Box::leak(Box::new(cloned));
        NonNull::from(leaked_data).cast()
    }
}

/// Get two Ptrs to a component C and compute the diff between them.
///
/// SAFETY: the data and other Ptr must be a valid pointer to a value of type C
unsafe fn erased_diff<C: Diffable>(
    previous_tick: Tick,
    previous: Ptr,
    present: Ptr,
) -> NonNull<u8> {
    // SAFETY: the data Ptr must be a valid pointer to a value of type C
    let delta = unsafe { C::diff(previous.deref::<C>(), present.deref::<C>()) };
    let delta_message = DeltaMessage {
        delta_type: DeltaType::Normal { previous_tick },
        delta,
    };
    // TODO: Box::leak seems incorrect here; use Box::into_raw()
    let leaked_data = Box::leak(Box::new(delta_message));
    NonNull::from(leaked_data).cast()
}

unsafe fn erased_base_diff<C: Diffable>(other: Ptr) -> NonNull<u8> {
    let base = C::base_value();
    // SAFETY: the data Ptr must be a valid pointer to a value of type C
    let delta = C::diff(&base, unsafe { other.deref::<C>() });
    let delta_message = DeltaMessage {
        delta_type: DeltaType::FromBase,
        delta,
    };
    let leaked_data = Box::leak(Box::new(delta_message));
    NonNull::from(leaked_data).cast()
}

/// SAFETY:
/// - the data PtrMut must be a valid pointer to a value of type C
/// - the delta Ptr must be a valid pointer to a value of type C::Delta
unsafe fn erased_apply_diff<C: Diffable>(data: PtrMut, delta: Ptr) {
    unsafe { C::apply_diff(data.deref_mut::<C>(), delta.deref::<C::Delta>()) };
}

unsafe fn erased_drop<C>(data: NonNull<u8>) {
    // reclaim the memory inside the box
    // the box's destructor will then free the memory and run drop
    let _ = unsafe { Box::from_raw(data.cast::<C>().as_ptr()) };
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ErasedDeltaFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    pub delta_kind: ComponentKind,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub clone: ErasedCloneFn,
    pub diff: ErasedDiffFn,
    pub diff_from_base: ErasedBaseDiffFn,
    pub apply_diff: ErasedApplyDiffFn,
    pub drop: ErasedDropFn,
    pub drop_delta_message: ErasedDropFn,
}

impl ErasedDeltaFns {
    pub(crate) fn new<C: Component + Diffable>() -> Self {
        Self {
            type_id: TypeId::of::<C>(),
            type_name: core::any::type_name::<C>(),
            delta_kind: ComponentKind::of::<DeltaMessage<C::Delta>>(),
            clone: erased_clone::<C>,
            diff: erased_diff::<C>,
            diff_from_base: erased_base_diff::<C>,
            apply_diff: erased_apply_diff::<C>,
            drop: erased_drop::<C>,
            drop_delta_message: erased_drop::<DeltaMessage<C::Delta>>,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "std"))]
    use alloc::{vec, vec::Vec};
    use bevy::platform::collections::HashSet;
    use bevy::prelude::Reflect;
    use serde::Deserialize;

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
    pub struct CompDelta(pub Vec<usize>);

    // NOTE: for the delta-compression to work, the components must have the same prefix, starting with [1]
    impl Diffable for CompDelta {
        // const IDEMPOTENT: bool = false;
        type Delta = Vec<usize>;

        fn base_value() -> Self {
            Self(vec![1])
        }

        fn diff(&self, other: &Self) -> Self::Delta {
            Vec::from_iter(other.0[self.0.len()..].iter().cloned())
        }

        fn apply_diff(&mut self, delta: &Self::Delta) {
            self.0.extend(delta);
        }
    }

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
    pub struct CompDelta2(pub HashSet<usize>);

    impl Diffable for CompDelta2 {
        // const IDEMPOTENT: bool = true;
        // additions, removals
        type Delta = (HashSet<usize>, HashSet<usize>);

        fn base_value() -> Self {
            Self(HashSet::default())
        }

        fn diff(&self, other: &Self) -> Self::Delta {
            let added = other.0.difference(&self.0).cloned().collect();
            let removed = self.0.difference(&other.0).cloned().collect();
            (added, removed)
        }

        fn apply_diff(&mut self, delta: &Self::Delta) {
            let (added, removed) = delta;
            self.0.extend(added);
            self.0.retain(|x| !removed.contains(x));
        }
    }

    #[test]
    fn test_erased_clone() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta>();
        let data = CompDelta(vec![1]);
        // clone data
        let cloned = unsafe { (erased_fns.clone)(Ptr::from(&data)) };
        // cast the ptr to the original type
        let casted = cloned.cast::<CompDelta>();
        assert_eq!(unsafe { casted.as_ref() }, &data);
        // free the leaked memory
        unsafe { (erased_fns.drop)(casted.cast()) };
        // NOTE: this doesn't work for some reason
        // unsafe { std::ptr::drop_in_place(casted.as_ptr()) };
    }

    // #[test]
    // #[should_panic]
    // fn test_erased_drop() {
    //     let erased_fns = ErasedDeltaFns::new::<Component6>();
    //     let mut data = Component6(vec![1]);
    //     assert!(core::mem::needs_drop::<Component6>());
    //     // drop data
    //     unsafe { (erased_fns.drop)(PtrMut::from(&mut data)) };
    //     // this panics because the memory has been freed
    //     assert_eq!(data, Component6(vec![1]));
    // }

    #[test]
    fn test_erased_diff() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta>();
        let old_data = CompDelta(vec![1]);
        let new_data = CompDelta(vec![1, 2]);

        let diff = old_data.diff(&new_data);
        assert_eq!(diff, vec![2]);

        let delta =
            unsafe { (erased_fns.diff)(Tick(0), Ptr::from(&old_data), Ptr::from(&new_data)) };
        let casted = delta.cast::<DeltaMessage<Vec<usize>>>();
        let delta_message = unsafe { casted.as_ref() };
        assert_eq!(
            delta_message.delta_type,
            DeltaType::Normal {
                previous_tick: Tick(0),
            }
        );
        assert_eq!(delta_message.delta, diff);
        // free memory
        unsafe {
            (erased_fns.drop_delta_message)(casted.cast());
        }
    }

    #[test]
    fn test_erased_from_base_diff() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta>();
        let new_data = CompDelta(vec![1, 2]);
        let delta = unsafe { (erased_fns.diff_from_base)(Ptr::from(&new_data)) };
        let casted = delta.cast::<DeltaMessage<Vec<usize>>>();
        let delta_message = unsafe { casted.as_ref() };
        assert_eq!(delta_message.delta_type, DeltaType::FromBase);
        assert_eq!(delta_message.delta, vec![2]);
        // free memory
        unsafe {
            (erased_fns.drop_delta_message)(casted.cast());
        }
    }

    #[test]
    fn test_apply_diff() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta>();
        let mut old_data = CompDelta(vec![1]);
        let diff: <CompDelta as Diffable>::Delta = vec![2];
        unsafe { (erased_fns.apply_diff)(PtrMut::from(&mut old_data), Ptr::from(&diff)) };
        assert_eq!(old_data, CompDelta(vec![1, 2]));
    }
}
