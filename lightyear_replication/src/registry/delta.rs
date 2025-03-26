use crate::shared::replication::delta::{DeltaComponentHistory, DeltaMessage, DeltaType, Diffable};

use crate::prelude::{ChannelDirection, ComponentRegistry, Tick};
use crate::protocol::component::replication::{RawWriteFn, ReplicationMetadata};
use crate::protocol::component::{ComponentError, ComponentKind};
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, format};
use bevy::ecs::component::Mutable;
use bevy::prelude::{Component, EntityWorldMut, World};
use bevy::ptr::{Ptr, PtrMut};
use core::any::TypeId;
use core::ptr::NonNull;
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
        let write: RawWriteFn = Self::write_delta::<C>;
        self.replication_map.insert(
            delta_kind,
            ReplicationMetadata {
                // Note: the direction should always exist; adding unwrap_or for unit tests
                direction: self
                    .replication_map
                    .get(&kind)
                    .map(|m| m.direction)
                    .unwrap_or(ChannelDirection::Bidirectional),
                write,
                buffer_insert_fn: Self::buffer_insert_delta::<C>,
                // we never need to remove the DeltaMessage<C> component
                remove: None,
            },
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
        self.erased_serialize( unsafe { Ptr::new(delta) }, writer, delta_fns.delta_kind, entity_map)?;
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
        let delta = unsafe { (delta_fns.diff_from_base)(component_data)};
        // SAFETY: the delta is a valid pointer to a DeltaMessage<C::Delta>
        self.erased_serialize(unsafe { Ptr::new(delta) }, writer, delta_fns.delta_kind, entity_map)?;
        // drop the delta message
        unsafe { (delta_fns.drop_delta_message)(delta) };
        Ok(())
    }

    /// Deserialize the DeltaMessage<C::Delta> and apply it to the component
    pub fn write_delta<C: Component<Mutability = Mutable> + PartialEq + Diffable>(
        &self,
        reader: &mut Reader,
        tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<(), ComponentError> {
        trace!(
            "Writing component delta {} to entity",
            core::any::type_name::<C>()
        );
        let kind = ComponentKind::of::<C>();
        let delta_net_id = self.net_id::<DeltaMessage<C::Delta>>();
        let delta = self.raw_deserialize::<DeltaMessage<C::Delta>>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        match delta.delta_type {
            DeltaType::Normal { previous_tick } => {
                let Some(mut history) = entity_world_mut.get_mut::<DeltaComponentHistory<C>>()
                else {
                    return Err(ComponentError::DeltaCompressionError(
                        format!("Entity {entity:?} does not have a ConfirmedHistory<{}>, but we received a diff for delta-compression",
                                core::any::type_name::<C>())
                    ));
                };
                let Some(past_value) = history.buffer.get(&previous_tick) else {
                    return Err(ComponentError::DeltaCompressionError(
                        format!("Entity {entity:?} does not have a value for tick {previous_tick:?} in the ConfirmedHistory<{}>",
                                core::any::type_name::<C>())
                    ));
                };
                // TODO: is it possible to have one clone instead of 2?
                let mut new_value = past_value.clone();
                new_value.apply_diff(&delta.delta);
                // we can remove all the values strictly older than previous_tick in the component history
                // (since we now know that the sender has received an ack for previous_tick, otherwise it wouldn't
                // have sent a diff based on the previous_tick)
                history.buffer = history.buffer.split_off(&previous_tick);
                // store the new value in the history
                history.buffer.insert(tick, new_value.clone());
                let Some(mut c) = entity_world_mut.get_mut::<C>() else {
                    return Err(ComponentError::DeltaCompressionError(
                        format!("Entity {entity:?} does not have a {} component, but we received a diff for delta-compression",
                        core::any::type_name::<C>())
                    ));
                };
                *c = new_value;

                // TODO: should we send the event based on the message type (Insert/Update) or based on whether the component was actually inserted?
                events.push_update_component(entity, kind, tick);
            }
            DeltaType::FromBase => {
                let mut new_value = C::base_value();
                new_value.apply_diff(&delta.delta);
                let value = new_value.clone();
                if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                    // only apply the update if the component is different, to not trigger change detection
                    if c.as_ref() != &new_value {
                        *c = new_value;
                        events.push_update_component(entity, kind, tick);
                    }
                } else {
                    entity_world_mut.insert(new_value);
                    events.push_insert_component(entity, kind, tick);
                }
                // store the component value in the delta component history, so that we can compute
                // diffs from it
                if let Some(mut history) = entity_world_mut.get_mut::<DeltaComponentHistory<C>>() {
                    history.buffer.insert(tick, value);
                } else {
                    // create a DeltaComponentHistory and insert the value
                    let mut history = DeltaComponentHistory::default();
                    history.buffer.insert(tick, value);
                    entity_world_mut.insert(history);
                }
            }
        }
        Ok(())
    }

    /// Insert a component delta into the entity.
    /// If the component is not present on the entity, we put it in a temporary buffer
    /// so that all components can be inserted at once
    pub fn buffer_insert_delta<C: Component<Mutability = Mutable> + PartialEq + Diffable>(
        &mut self,
        reader: &mut Reader,
        tick: Tick,
        entity_world_mut: &mut EntityWorldMut,
        entity_map: &mut ReceiveEntityMap,
        events: &mut ConnectionEvents,
    ) -> Result<(), ComponentError> {
        let kind = ComponentKind::of::<C>();
        let component_id = self
            .kind_to_component_id
            .get(&kind)
            .ok_or(ComponentError::NotRegistered)?;
        trace!(
            ?kind,
            ?component_id,
            "Writing component delta {} to entity",
            core::any::type_name::<C>()
        );
        let delta = self.raw_deserialize::<DeltaMessage<C::Delta>>(reader, entity_map)?;
        let entity = entity_world_mut.id();
        match delta.delta_type {
            DeltaType::Normal { previous_tick } => {
                unreachable!("buffer_insert_delta should only be called for FromBase deltas since the component is being inserted");
            }
            DeltaType::FromBase => {
                let mut new_value = C::base_value();
                new_value.apply_diff(&delta.delta);
                // clone the value so that we can insert it in the history
                let cloned_value = new_value.clone();

                // if the component is on the entity, no need to insert
                if let Some(mut c) = entity_world_mut.get_mut::<C>() {
                    // only apply the update if the component is different, to not trigger change detection
                    if c.as_ref() != &new_value {
                        *c = new_value;
                        events.push_update_component(entity, kind, tick);
                    }
                } else {
                    // TODO: add safety comment
                    // use the component id of C, not DeltaMessage<C>
                    unsafe {
                        self.temp_write_buffer
                            .buffer_insert_raw_ptrs::<C>(new_value, *component_id)
                    };
                    events.push_insert_component(entity, kind, tick);
                }
                // store the component value in the delta component history, so that we can compute
                // diffs from it
                if let Some(mut history) = entity_world_mut.get_mut::<DeltaComponentHistory<C>>() {
                    history.buffer.insert(tick, cloned_value);
                } else {
                    // create a DeltaComponentHistory and insert the value
                    let mut history = DeltaComponentHistory::default();
                    history.buffer.insert(tick, cloned_value);
                    entity_world_mut.insert(history);
                }
            }
        }
        Ok(())
    }
}

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedDiffFn = unsafe fn(start_tick: Tick, start: Ptr, present: Ptr) -> NonNull<u8>;
type ErasedBaseDiffFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);
type ErasedDropFn = unsafe fn(data: NonNull<u8>);

/// SAFETY: the Ptr must be a valid pointer to a value of type C
unsafe fn erased_clone<C: Clone>(data: Ptr) -> NonNull<u8> { unsafe {
    let cloned: C = data.deref::<C>().clone();
    let leaked_data = Box::leak(Box::new(cloned));
    NonNull::from(leaked_data).cast()
}}

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
    unsafe { C::apply_diff(data.deref_mut::<C>(), delta.deref::<C::Delta>()) } ;
}

unsafe fn erased_drop<C>(data: NonNull<u8>) {
    // reclaim the memory inside the box
    // the box's destructor will then free the memory and run drop
    let _ = unsafe { Box::from_raw(data.cast::<C>().as_ptr()) } ;
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
    use crate::tests::protocol::ComponentDeltaCompression;
    #[cfg(not(feature = "std"))]
    use alloc::{vec, vec::Vec};

    #[test]
    fn test_erased_clone() {
        let erased_fns = ErasedDeltaFns::new::<ComponentDeltaCompression>();
        let data = ComponentDeltaCompression(vec![1]);
        // clone data
        let cloned = unsafe { (erased_fns.clone)(Ptr::from(&data)) };
        // cast the ptr to the original type
        let casted = cloned.cast::<ComponentDeltaCompression>();
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
        let erased_fns = ErasedDeltaFns::new::<ComponentDeltaCompression>();
        let old_data = ComponentDeltaCompression(vec![1]);
        let new_data = ComponentDeltaCompression(vec![1, 2]);

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
        let erased_fns = ErasedDeltaFns::new::<ComponentDeltaCompression>();
        let new_data = ComponentDeltaCompression(vec![1, 2]);
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
        let erased_fns = ErasedDeltaFns::new::<ComponentDeltaCompression>();
        let mut old_data = ComponentDeltaCompression(vec![1]);
        let diff: <ComponentDeltaCompression as Diffable>::Delta = vec![2];
        unsafe { (erased_fns.apply_diff)(PtrMut::from(&mut old_data), Ptr::from(&diff)) };
        assert_eq!(old_data, ComponentDeltaCompression(vec![1, 2]));
    }
}
