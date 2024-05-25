use crate::prelude::Message;
use crate::protocol::component::ComponentKind;
use crate::protocol::serialize::ErasedMapEntitiesFn;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::shared::replication::delta::Diffable;
use bevy::prelude::Component;
use bevy::ptr::{Ptr, PtrMut};
use std::any::TypeId;
use std::ptr::NonNull;

type ErasedCloneFn = unsafe fn(data: Ptr) -> NonNull<u8>;

type ErasedDiffFn = unsafe fn(data: Ptr, other: Ptr) -> NonNull<u8>;

type ErasedApplyDiffFn = unsafe fn(data: PtrMut, delta: Ptr);

type ErasedBaseValueFn = unsafe fn() -> NonNull<u8>;

/// SAFETY: the Ptr must be a valid pointer to a value of type C
unsafe fn erased_clone<C: Clone>(data: Ptr) -> NonNull<u8> {
    let mut cloned = data.deref::<C>().clone();
    let cloned_ptr = &mut cloned as *mut C;
    let cloned_ptr = cloned_ptr.cast::<u8>();
    let owned_ptr = NonNull::new(cloned_ptr).unwrap();
    std::mem::forget(cloned);
    owned_ptr
}

/// SAFETY: the data and other Ptr must be a valid pointer to a value of type C
unsafe fn erased_diff<C: Diffable>(data: Ptr, other: Ptr) -> NonNull<u8> {
    let mut delta = C::diff(data.deref::<C>(), other.deref::<C>());
    let delta_ptr = &mut delta as *mut C::Delta;
    let delta_ptr = delta_ptr.cast::<u8>();
    NonNull::new(delta_ptr).unwrap()
}

/// SAFETY:
/// - the data PtrMut must be a valid pointer to a value of type C
/// - the delta Ptr must be a valid pointer to a value of type C::Delta
unsafe fn erased_apply_diff<C: Diffable>(data: PtrMut, delta: Ptr) {
    C::apply_diff(data.deref_mut::<C>(), delta.deref::<C::Delta>());
}

unsafe fn erased_base_value<C: Diffable>() -> NonNull<u8> {
    let mut base = C::base_value();
    let base_ptr = &mut base as *mut C;
    let base_ptr = base_ptr.cast::<u8>();
    let base_ptr = NonNull::new(base_ptr).unwrap();
    std::mem::forget(base);
    base_ptr
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ErasedDeltaFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    pub delta_kind: ComponentKind,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub clone: ErasedCloneFn,
    pub diff: ErasedDiffFn,
    pub apply_diff: ErasedApplyDiffFn,
    pub base_value: ErasedBaseValueFn,
}

impl ErasedDeltaFns {
    pub(crate) fn new<C: Component + Diffable>() -> Self {
        Self {
            type_id: TypeId::of::<C>(),
            type_name: std::any::type_name::<C>(),
            delta_kind: ComponentKind::of::<C::Delta>(),
            clone: erased_clone::<C>,
            diff: erased_diff::<C>,
            apply_diff: erased_apply_diff::<C>,
            base_value: erased_base_value::<C>,
        }
    }
}
