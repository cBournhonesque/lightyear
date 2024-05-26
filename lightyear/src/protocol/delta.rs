use crate::prelude::Message;
use crate::protocol::component::ComponentKind;
use crate::protocol::serialize::ErasedMapEntitiesFn;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::shared::replication::delta::{DeltaMessage, DeltaType, Diffable};
use bevy::prelude::Component;
use bevy::ptr::{Ptr, PtrMut};
use std::any::TypeId;
use std::ptr::NonNull;

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;

type ErasedDiffFn = unsafe fn(data: Ptr, other: Ptr) -> NonNull<u8>;
type ErasedBaseDiffFn = unsafe fn(data: Ptr) -> NonNull<u8>;

type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);

type ErasedDropFn = unsafe fn(data: PtrMut);

/// SAFETY: the Ptr must be a valid pointer to a value of type C
unsafe fn erased_clone<C: Clone>(data: Ptr) -> NonNull<u8> {
    let mut cloned = data.deref::<C>().clone();
    let cloned_ptr = &mut cloned as *mut C;
    let cloned_ptr = cloned_ptr.cast::<u8>();
    let owned_ptr = NonNull::new(cloned_ptr).unwrap();
    std::mem::forget(cloned);
    owned_ptr
}

/// Get two Ptrs to a component C and compute the diff between them.
///
/// SAFETY: the data and other Ptr must be a valid pointer to a value of type C
unsafe fn erased_diff<C: Diffable>(data: Ptr, other: Ptr) -> NonNull<u8> {
    let delta = C::diff(data.deref::<C>(), other.deref::<C>());
    let mut delta_message = DeltaMessage {
        delta_type: DeltaType::Normal,
        delta,
    };
    let delta_ptr = &mut delta_message as *mut DeltaMessage<C::Delta>;
    let delta_ptr = delta_ptr.cast::<u8>();
    std::mem::forget(delta_message);
    NonNull::new(delta_ptr).unwrap()
}

unsafe fn erased_base_diff<C: Diffable>(other: Ptr) -> NonNull<u8> {
    let base = C::base_value();
    let delta = C::diff(&base, other.deref::<C>());
    let mut delta_message = DeltaMessage {
        delta_type: DeltaType::FromBase,
        delta,
    };
    let base_ptr = &mut delta_message as *mut DeltaMessage<C::Delta>;
    let base_ptr = base_ptr.cast::<u8>();
    let base_ptr = NonNull::new(base_ptr).unwrap();
    std::mem::forget(delta_message);
    base_ptr
}

/// SAFETY:
/// - the data PtrMut must be a valid pointer to a value of type C
/// - the delta Ptr must be a valid pointer to a value of type C::Delta
unsafe fn erased_apply_diff<C: Diffable>(data: PtrMut, delta: Ptr) {
    C::apply_diff(data.deref_mut::<C>(), delta.deref::<C::Delta>());
}

unsafe fn erased_drop<C>(data: PtrMut) {
    let data = data.deref_mut::<C>();
    std::ptr::drop_in_place(data);
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
