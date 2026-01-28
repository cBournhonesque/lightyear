#![allow(clippy::collapsible_else_if)]
use crate::components::Confirmed;
use crate::delta::{DeltaComponentHistory, DeltaMessage, DeltaType, Diffable};
use crate::prelude::ComponentReplicationConfig;
use crate::registry::buffered::BufferedEntity;
use crate::registry::registry::ComponentRegistry;
use crate::registry::replication::ReplicationMetadata;
use crate::registry::{ComponentError, ComponentKind};
use alloc::boxed::Box;
use alloc::format;
use bevy_ecs::{
    component::{Component, ComponentId, Mutable},
    world::World,
};
use bevy_ptr::{Ptr, PtrMut};
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use core::ptr::NonNull;
use lightyear_core::tick::Tick;
use lightyear_messages::Message;
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{CloneFn, ContextDeserializeFns};
use lightyear_serde::writer::Writer;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::trace;

impl ComponentRegistry {
    /// Register delta compression functions for a component
    pub fn set_delta_compression<
        C: Component<Mutability = Mutable> + PartialEq + Diffable<Delta>,
        Delta,
    >(
        &mut self,
        world: &mut World,
    ) where
        Delta: Serialize + DeserializeOwned + Message,
    {
        let kind = ComponentKind::of::<C>();
        let delta_kind = ComponentKind::of::<DeltaMessage<Delta>>();

        // add delta-related type-erased functions for C
        let metadata = self
            .component_metadata_map
            .get_mut(&kind)
            .unwrap_or_else(|| {
                panic!(
                    "Can only add delta-compression on a registered component (kind = {:?}).",
                    DebugName::type_name::<C>()
                );
            });
        metadata.delta = Some(ErasedDeltaFns::new::<C, Delta>());

        let mut predicted = false;
        let mut interpolated = false;
        // update the write function to use the delta compression logic
        if let Some(replication) = metadata.replication.as_mut() {
            replication.config.delta_compression = true;
            predicted = replication.predicted;
            interpolated = replication.interpolated;
        }

        // add serialization/replication for C::Delta
        self.register_component::<DeltaMessage<Delta>>(world);

        // add write/remove functions associated with the delta component's net_id
        // (since the serialized message will contain the delta component's net_id)
        let delta_metadata = self.component_metadata_map.get_mut(&delta_kind).unwrap();
        let mut new_metadata = ReplicationMetadata::new(
            ComponentReplicationConfig::default(),
            ComponentId::new(0),
            buffer_insert_delta::<C, Delta>,
        );
        // TODO: delta compression must be applied AFTER prediction/interpolation
        new_metadata.set_predicted(predicted);
        new_metadata.set_interpolated(interpolated);
        delta_metadata.replication = Some(new_metadata);
    }

    /// # Safety
    /// the Ptr must correspond to the correct ComponentKind
    pub unsafe fn erased_clone(
        &self,
        data: Ptr,
        kind: ComponentKind,
    ) -> Result<NonNull<u8>, ComponentError> {
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
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
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
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
        trace!(
            ?kind,
            "Serializing diff from previous value for delta component",
        );
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
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
        trace!(
            ?kind,
            "Serializing diff from base value for delta component",
        );
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
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

    /// Compute diff from base value and return both the serialized data and the reconstructed
    /// component value (base + delta). This is used for server-side reconstruction tracking
    /// to prevent quantization drift.
    ///
    /// Returns (serialized_bytes, reconstructed_component_ptr)
    ///
    /// # Safety
    /// The component_data Ptr must correspond to the correct ComponentKind
    pub unsafe fn serialize_diff_from_base_value_with_reconstruction(
        &self,
        component_data: Ptr,
        writer: &mut Writer,
        kind: ComponentKind,
        entity_map: &mut SendEntityMap,
    ) -> Result<NonNull<u8>, ComponentError> {
        trace!(
            ?kind,
            "Serializing diff from base value with reconstruction for delta component",
        );
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
            .ok_or(ComponentError::MissingDeltaFns)?;

        // Compute the delta from base to current value
        let delta = unsafe { (delta_fns.diff_from_base)(component_data) };

        // Serialize the delta
        self.erased_serialize(
            unsafe { Ptr::new(delta) },
            writer,
            delta_fns.delta_kind,
            entity_map,
        )?;

        // Create reconstructed value: base + delta (what client will have)
        let reconstructed = unsafe { (delta_fns.create_base_with_delta)(Ptr::new(delta)) };

        // Drop the delta message
        unsafe { (delta_fns.drop_delta_message)(delta) };

        Ok(reconstructed)
    }

    /// Compute diff from a previous value and return both the serialized data and apply the
    /// delta to the stored baseline for server-side reconstruction tracking.
    ///
    /// This modifies the stored baseline in-place by applying the same delta the client will apply.
    ///
    /// # Safety
    /// - start and new Ptrs must correspond to the correct ComponentKind
    /// - stored_baseline must be a mutable pointer to a value that can be modified
    pub unsafe fn serialize_diff_with_reconstruction(
        &self,
        start_tick: Tick,
        start: Ptr,
        new: Ptr,
        stored_baseline: PtrMut,
        writer: &mut Writer,
        kind: ComponentKind,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), ComponentError> {
        trace!(
            ?kind,
            "Serializing diff with reconstruction for delta component",
        );
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
            .ok_or(ComponentError::MissingDeltaFns)?;

        // Compute the delta from old to new
        let delta = unsafe { (delta_fns.diff)(start_tick, start, new) };

        // Serialize the delta
        self.erased_serialize(
            unsafe { Ptr::new(delta) },
            writer,
            delta_fns.delta_kind,
            entity_map,
        )?;

        // Apply the same delta to the stored baseline (server-side reconstruction)
        // Extract just the delta part from the DeltaMessage
        let delta_ptr = unsafe {
            // Offset to get to the delta field within DeltaMessage
            // DeltaMessage is repr(C), so delta_type comes first, then delta
            let delta_message_ptr = delta.cast::<u8>().as_ptr();
            // Size of DeltaType enum (it's a simple enum with two variants, so typically 1-8 bytes)
            let delta_offset = core::mem::size_of::<crate::delta::DeltaType>();
            Ptr::new(NonNull::new_unchecked(delta_message_ptr.add(delta_offset)))
        };
        unsafe { (delta_fns.apply_diff)(stored_baseline, delta_ptr) };

        // Drop the delta message
        unsafe { (delta_fns.drop_delta_message)(delta) };

        Ok(())
    }

    /// Apply a delta to stored component data (for server-side reconstruction tracking).
    ///
    /// # Safety
    /// - data must be a valid mutable pointer to a component of the given kind
    /// - delta must be a valid pointer to the delta type for this component
    pub unsafe fn erased_apply_diff(
        &self,
        data: PtrMut,
        delta: Ptr,
        kind: ComponentKind,
    ) -> Result<(), ComponentError> {
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
            .ok_or(ComponentError::MissingDeltaFns)?;
        unsafe { (delta_fns.apply_diff)(data, delta) };
        Ok(())
    }

    /// Create a base value for a component type.
    ///
    /// # Safety
    /// Caller must ensure the returned pointer is eventually dropped via erased_drop.
    pub unsafe fn erased_create_base(&self, kind: ComponentKind) -> Result<NonNull<u8>, ComponentError> {
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
            .ok_or(ComponentError::MissingDeltaFns)?;
        Ok(unsafe { (delta_fns.create_base)() })
    }

    /// Create a component value by applying a delta to the base value.
    ///
    /// # Safety
    /// - delta must be a valid pointer to a DeltaMessage for this component's delta type
    /// - Caller must ensure the returned pointer is eventually dropped via erased_drop.
    pub unsafe fn erased_create_base_with_delta(
        &self,
        delta: Ptr,
        kind: ComponentKind,
    ) -> Result<NonNull<u8>, ComponentError> {
        let delta_fns = self
            .component_metadata_map
            .get(&kind)
            .and_then(|m| m.delta.as_ref())
            .ok_or(ComponentError::MissingDeltaFns)?;
        Ok(unsafe { (delta_fns.create_base_with_delta)(delta) })
    }
}

/// Insert a component delta into the entity.
/// If the component is not present on the entity, we put it in a temporary buffer
/// so that all components can be inserted at once
fn buffer_insert_delta<
    C: Component<Mutability = Mutable> + PartialEq + Diffable<Delta>,
    Delta: Message,
>(
    deserialize: ContextDeserializeFns<ReceiveEntityMap, DeltaMessage<Delta>, DeltaMessage<Delta>>,
    clone: Option<CloneFn<C>>,
    reader: &mut Reader,
    tick: Tick,
    entity_mut: &mut BufferedEntity,
    entity_map: &mut ReceiveEntityMap,
    predicted: bool,
    interpolated: bool,
) -> Result<(), ComponentError> {
    let kind = ComponentKind::of::<C>();
    let component_id = entity_mut.component_id::<C>();

    let entity = entity_mut.entity.id();
    let delta = deserialize.deserialize(entity_map, reader)?;
    trace!(
        ?tick,
        ?predicted,
        ?interpolated,
        ?kind,
        ?component_id,
        delta_type = ?delta.delta_type,
        "Writing component delta {} to entity",
        DebugName::type_name::<C>()
    );
    match delta.delta_type {
        DeltaType::Normal { previous_tick } => {
            let Some(mut history) = entity_mut.entity.get_mut::<DeltaComponentHistory<C>>() else {
                return Err(ComponentError::DeltaCompressionError(format!(
                    "Entity {entity:?} does not have a ConfirmedHistory<{}>, but we received a diff for delta-compression",
                    DebugName::type_name::<C>()
                )));
            };
            let Some(past_value) = history.buffer.get(&previous_tick) else {
                return Err(ComponentError::DeltaCompressionError(format!(
                    "Entity {entity:?} does not have a value for tick {previous_tick:?} in the ConfirmedHistory<{}>",
                    DebugName::type_name::<C>()
                )));
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
            if predicted || interpolated {
                let Some(mut c) = entity_mut.entity.get_mut::<Confirmed<C>>() else {
                    return Err(ComponentError::DeltaCompressionError(format!(
                        "Entity {entity:?} does not have a {} component, but we received a diff for delta-compression",
                        DebugName::type_name::<C>()
                    )));
                };
                *c = Confirmed(new_value);
            } else {
                let Some(mut c) = entity_mut.entity.get_mut::<C>() else {
                    return Err(ComponentError::DeltaCompressionError(format!(
                        "Entity {entity:?} does not have a {} component, but we received a diff for delta-compression",
                        DebugName::type_name::<C>()
                    )));
                };
                *c = new_value;
            }
        }
        DeltaType::FromBase => {
            let mut new_value = C::base_value();
            new_value.apply_diff(&delta.delta);
            // clone the value so that we can insert it in the history
            let cloned_value = new_value.clone();

            // if the component is on the entity, no need to insert
            if predicted || interpolated {
                let new_value = Confirmed(new_value);
                if let Some(mut c) = entity_mut.entity.get_mut::<Confirmed<C>>() {
                    // only apply the update if the component is different, to not trigger change detection
                    if c.as_ref() != &new_value {
                        *c = new_value;
                    }
                } else {
                    trace!(
                        ?entity,
                        "Insert Confirmed<{:?}>",
                        DebugName::type_name::<C>()
                    );
                    let confirmed_component_id = entity_mut.component_id::<Confirmed<C>>();
                    if predicted && !entity_mut.entity.contains::<C>() {
                        let cloned = clone.unwrap()(&new_value.0);
                        // SAFETY: we made sure that component_id corresponds to C
                        unsafe {
                            entity_mut.buffered.insert::<C>(cloned, component_id);
                        }
                    }
                    // use the component id of C, not DeltaMessage<C>
                    // SAFETY: we are inserting a component of type Confirmed<C>, which matches the confirmed_component_id
                    unsafe {
                        entity_mut
                            .buffered
                            .insert::<Confirmed<C>>(new_value, confirmed_component_id);
                    }
                }
            } else {
                if let Some(mut c) = entity_mut.entity.get_mut::<C>() {
                    // only apply the update if the component is different, to not trigger change detection
                    if c.as_ref() != &new_value {
                        *c = new_value;
                    }
                } else {
                    // use the component id of C, not DeltaMessage<C>
                    // SAFETY: we are inserting a component of type C, which matches the component_id
                    unsafe {
                        entity_mut.buffered.insert::<C>(new_value, component_id);
                    }
                }
            }

            // store the component value in the delta component history, so that we can compute
            // diffs from it
            entity_mut
                .entity
                .entry::<DeltaComponentHistory<C>>()
                .or_default()
                .get_mut()
                .buffer
                .insert(tick, cloned_value);
        }
    }
    Ok(())
}

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedDiffFn = unsafe fn(start_tick: Tick, start: Ptr, present: Ptr) -> NonNull<u8>;
type ErasedBaseDiffFn = unsafe fn(data: Ptr) -> NonNull<u8>;
type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);
type ErasedDropFn = unsafe fn(data: NonNull<u8>);
type ErasedCreateBaseFn = unsafe fn() -> NonNull<u8>;
/// Returns (component_ptr, delta_ptr) where component is base + delta
type ErasedCreateBaseWithDeltaFn = unsafe fn(delta: Ptr) -> NonNull<u8>;

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
unsafe fn erased_diff<C: Diffable<Delta>, Delta>(
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
    let leaked_data = Box::into_raw(Box::new(delta_message));
    // SAFETY: we know from above that leaked_data is not null
    unsafe { NonNull::new_unchecked(leaked_data).cast() }
}

unsafe fn erased_base_diff<C: Diffable<Delta>, Delta>(other: Ptr) -> NonNull<u8> {
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
unsafe fn erased_apply_diff<C: Diffable<Delta>, Delta>(data: PtrMut, delta: Ptr) {
    unsafe { C::apply_diff(data.deref_mut::<C>(), delta.deref::<Delta>()) };
}

unsafe fn erased_drop<C>(data: NonNull<u8>) {
    // reclaim the memory inside the box
    // the box's destructor will then free the memory and run drop
    let _ = unsafe { Box::from_raw(data.cast::<C>().as_ptr()) };
}

/// Create a base value for a component type.
///
/// SAFETY: Caller must ensure the returned pointer is eventually dropped via erased_drop.
unsafe fn erased_create_base<C: Diffable<Delta>, Delta>() -> NonNull<u8> {
    let base = C::base_value();
    let leaked = Box::leak(Box::new(base));
    NonNull::from(leaked).cast()
}

/// Create a component value by applying a delta to the base value.
/// This is used for server-side reconstruction tracking to prevent quantization drift.
///
/// SAFETY:
/// - delta must be a valid pointer to a DeltaMessage<Delta>
/// - Caller must ensure the returned pointer is eventually dropped via erased_drop.
unsafe fn erased_create_base_with_delta<C: Diffable<Delta>, Delta>(
    delta: Ptr,
) -> NonNull<u8> {
    let mut base = C::base_value();
    // Extract delta from DeltaMessage wrapper
    let delta_message = unsafe { delta.deref::<DeltaMessage<Delta>>() };
    base.apply_diff(&delta_message.delta);
    let leaked = Box::leak(Box::new(base));
    NonNull::from(leaked).cast()
}

#[derive(Debug, Clone)]
pub(crate) struct ErasedDeltaFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: DebugName,
    pub delta_kind: ComponentKind,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub clone: ErasedCloneFn,
    pub diff: ErasedDiffFn,
    pub diff_from_base: ErasedBaseDiffFn,
    pub apply_diff: ErasedApplyDiffFn,
    pub drop: ErasedDropFn,
    pub drop_delta_message: ErasedDropFn,
    /// Create a base value for the component type
    pub create_base: ErasedCreateBaseFn,
    /// Create a component by applying delta to base (for server-side reconstruction tracking)
    pub create_base_with_delta: ErasedCreateBaseWithDeltaFn,
}

impl ErasedDeltaFns {
    pub(crate) fn new<C: Component + Diffable<Delta>, Delta: Message>() -> Self {
        Self {
            type_id: TypeId::of::<C>(),
            type_name: DebugName::type_name::<C>(),
            delta_kind: ComponentKind::of::<DeltaMessage<Delta>>(),
            clone: erased_clone::<C>,
            diff: erased_diff::<C, Delta>,
            diff_from_base: erased_base_diff::<C, Delta>,
            apply_diff: erased_apply_diff::<C, Delta>,
            drop: erased_drop::<C>,
            drop_delta_message: erased_drop::<DeltaMessage<Delta>>,
            create_base: erased_create_base::<C, Delta>,
            create_base_with_delta: erased_create_base_with_delta::<C, Delta>,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::{vec, vec::Vec};
    use bevy_ecs::component::Component;
    use bevy_platform::collections::HashSet;
    use bevy_reflect::Reflect;
    use serde::Deserialize;

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
    pub struct CompDelta(pub Vec<usize>);

    // NOTE: for the delta-compression to work, the components must have the same prefix, starting with [1]
    impl Diffable<Vec<usize>> for CompDelta {
        fn base_value() -> Self {
            Self(vec![1])
        }

        fn diff(&self, other: &Self) -> Vec<usize> {
            Vec::from_iter(other.0[self.0.len()..].iter().cloned())
        }

        fn apply_diff(&mut self, delta: &Vec<usize>) {
            self.0.extend(delta);
        }
    }

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
    pub struct CompDelta2(pub HashSet<usize>);

    // additions, removals
    impl Diffable<(HashSet<usize>, HashSet<usize>)> for CompDelta2 {
        fn base_value() -> Self {
            Self(HashSet::default())
        }

        fn diff(&self, other: &Self) -> (HashSet<usize>, HashSet<usize>) {
            let added = other.0.difference(&self.0).cloned().collect();
            let removed = self.0.difference(&other.0).cloned().collect();
            (added, removed)
        }

        fn apply_diff(&mut self, delta: &(HashSet<usize>, HashSet<usize>)) {
            let (added, removed) = delta;
            self.0.extend(added);
            self.0.retain(|x| !removed.contains(x));
        }
    }

    #[test]
    fn test_erased_clone() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();
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
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();
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
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();
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
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();
        let mut old_data = CompDelta(vec![1]);
        let diff = vec![2];
        unsafe { (erased_fns.apply_diff)(PtrMut::from(&mut old_data), Ptr::from(&diff)) };
        assert_eq!(old_data, CompDelta(vec![1, 2]));
    }

    #[test]
    fn test_create_base() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();
        let base = unsafe { (erased_fns.create_base)() };
        let casted = base.cast::<CompDelta>();
        assert_eq!(unsafe { casted.as_ref() }, &CompDelta::base_value());
        // free memory
        unsafe { (erased_fns.drop)(casted.cast()) };
    }

    #[test]
    fn test_create_base_with_delta() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();

        // Use the actual diff_from_base function to create the delta message
        // This matches production usage exactly
        let new_data = CompDelta(vec![1, 2, 3]);
        let delta = unsafe { (erased_fns.diff_from_base)(Ptr::from(&new_data)) };

        // Create base + delta (what client will reconstruct)
        let reconstructed = unsafe { (erased_fns.create_base_with_delta)(Ptr::new(delta)) };
        let casted = reconstructed.cast::<CompDelta>();

        // base is vec![1], new_data is vec![1, 2, 3], so delta is vec![2, 3]
        // Reconstructed should be base + delta = vec![1] + vec![2, 3] = vec![1, 2, 3]
        assert_eq!(unsafe { casted.as_ref() }, &CompDelta(vec![1, 2, 3]));

        // free memory
        unsafe { (erased_fns.drop)(casted.cast()) };
        unsafe { (erased_fns.drop_delta_message)(delta) };
    }

    /// Test that server-side reconstruction tracking prevents drift.
    /// This simulates the scenario where:
    /// 1. Server computes delta from stored baseline to new value
    /// 2. Server applies that same delta to its stored baseline
    /// 3. Client applies delta to its stored baseline
    /// Both should end up with identical values.
    #[test]
    fn test_reconstruction_tracking_prevents_drift() {
        let erased_fns = ErasedDeltaFns::new::<CompDelta, Vec<usize>>();

        // Initial state: both server and client have base value
        let mut server_baseline = CompDelta::base_value(); // vec![1]
        let mut client_baseline = CompDelta::base_value(); // vec![1]

        // Server has new true value
        let server_true_value = CompDelta(vec![1, 2, 3]);

        // Server computes delta from baseline to true value
        let delta = server_baseline.diff(&server_true_value);
        assert_eq!(delta, vec![2, 3]);

        // Server applies delta to its baseline (reconstruction tracking)
        server_baseline.apply_diff(&delta);

        // Client applies the same delta to its baseline
        client_baseline.apply_diff(&delta);

        // Both should be identical
        assert_eq!(server_baseline, client_baseline);
        assert_eq!(server_baseline, CompDelta(vec![1, 2, 3]));

        // Now simulate another update
        let server_true_value_2 = CompDelta(vec![1, 2, 3, 4, 5]);

        // Server computes delta from its RECONSTRUCTED baseline (not true value!)
        let delta_2 = server_baseline.diff(&server_true_value_2);
        assert_eq!(delta_2, vec![4, 5]);

        // Both apply the delta
        server_baseline.apply_diff(&delta_2);
        client_baseline.apply_diff(&delta_2);

        // Both should still be identical
        assert_eq!(server_baseline, client_baseline);
        assert_eq!(server_baseline, CompDelta(vec![1, 2, 3, 4, 5]));
    }
}
