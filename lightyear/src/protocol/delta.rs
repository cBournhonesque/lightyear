use crate::prelude::Message;
use crate::protocol::component::ComponentKind;
use crate::protocol::serialize::ErasedMapEntitiesFn;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::shared::replication::delta::{DeltaMessage, DeltaType, Diffable};
use bevy::prelude::Component;
use bevy::ptr::{Ptr, PtrMut};
use std::any::TypeId;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;

type ErasedDiffFn = unsafe fn(data: Ptr, other: Ptr) -> NonNull<u8>;
type ErasedBaseDiffFn = unsafe fn(data: Ptr) -> NonNull<u8>;

type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);

type ErasedDropFn = unsafe fn(data: NonNull<u8>);

/// SAFETY: the Ptr must be a valid pointer to a value of type C
unsafe fn erased_clone<C: Clone>(data: Ptr) -> NonNull<u8> {
    let cloned: C = data.deref::<C>().clone();
    let leaked_data = Box::leak(Box::new(cloned));
    NonNull::from(leaked_data).cast()
}

/// Get two Ptrs to a component C and compute the diff between them.
///
/// SAFETY: the data and other Ptr must be a valid pointer to a value of type C
unsafe fn erased_diff<C: Diffable>(old: Ptr, new: Ptr) -> NonNull<u8> {
    let delta = C::diff(old.deref::<C>(), new.deref::<C>());
    let delta_message = DeltaMessage {
        delta_type: DeltaType::Normal,
        delta,
    };
    let leaked_data = Box::leak(Box::new(delta_message));
    NonNull::from(leaked_data).cast()
}

unsafe fn erased_base_diff<C: Diffable>(other: Ptr) -> NonNull<u8> {
    let base = C::base_value();
    let delta = C::diff(&base, other.deref::<C>());
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
    C::apply_diff(data.deref_mut::<C>(), delta.deref::<C::Delta>());
}

unsafe fn erased_drop<C>(data: NonNull<u8>) {
    // reclaim the memory inside the box
    // the box's destructor will then free the memory and run drop
    Box::from_raw(data.cast::<C>().as_ptr());
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
            type_name: std::any::type_name::<C>(),
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
    use crate::tests::protocol::Component6;

    #[test]
    fn test_erased_clone() {
        let erased_fns = ErasedDeltaFns::new::<Component6>();
        let data = Component6(vec![1]);
        // clone data
        let cloned = unsafe { (erased_fns.clone)(Ptr::from(&data)) };
        // cast the ptr to the original type
        let casted = cloned.cast::<Component6>();
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
    //     assert!(std::mem::needs_drop::<Component6>());
    //     // drop data
    //     unsafe { (erased_fns.drop)(PtrMut::from(&mut data)) };
    //     // this panics because the memory has been freed
    //     assert_eq!(data, Component6(vec![1]));
    // }

    #[test]
    fn test_erased_diff() {
        let erased_fns = ErasedDeltaFns::new::<Component6>();
        let old_data = Component6(vec![1]);
        let new_data = Component6(vec![1, 2]);

        let diff = old_data.diff(&new_data);
        assert_eq!(diff, vec![2]);

        let delta = unsafe { (erased_fns.diff)(Ptr::from(&old_data), Ptr::from(&new_data)) };
        let casted = delta.cast::<DeltaMessage<Vec<usize>>>();
        let delta_message = unsafe { casted.as_ref() };
        assert_eq!(delta_message.delta_type, DeltaType::Normal);
        assert_eq!(delta_message.delta, diff);
        // free memory
        unsafe {
            (erased_fns.drop_delta_message)(casted.cast());
        }
    }

    #[test]
    fn test_erased_from_base_diff() {
        let erased_fns = ErasedDeltaFns::new::<Component6>();
        let new_data = Component6(vec![1, 2]);
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
}
