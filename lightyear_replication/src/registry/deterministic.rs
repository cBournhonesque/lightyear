use crate::prelude::{ComponentRegistration, ComponentRegistry};
use crate::registry::ComponentKind;
use crate::registry::registry::ComponentMetadata;
use bevy_ecs::change_detection::Mut;
use bevy_ecs::component::Component;
use bevy_ptr::Ptr;
use core::fmt::Debug;
use tracing::trace;

#[derive(Debug, Clone, Copy)]
pub struct DeterministicFns {
    // function fn(&C, &mut seahash::SeaHasher) converted to unsafe fn() to avoid generic parameters in enum
    pub inner: fn(),
    hash_fn: fn(Ptr, &mut seahash::SeaHasher, unsafe fn()),
}

impl DeterministicFns {
    pub fn new<C: Debug>(inner: fn(&C, &mut seahash::SeaHasher)) -> Self {
        DeterministicFns {
            inner: unsafe { core::mem::transmute::<fn(&C, &mut seahash::SeaHasher), fn()>(inner) },
            hash_fn: custom_hash_fn::<C>,
        }
    }

    pub fn hash_component(&self, ptr: Ptr, hasher: &mut seahash::SeaHasher) {
        (self.hash_fn)(ptr, hasher, self.inner);
    }
}

fn custom_hash_fn<C: Debug>(ptr: Ptr, hasher: &mut seahash::SeaHasher, f: unsafe fn()) {
    let f = unsafe { core::mem::transmute::<_, fn(&C, &mut seahash::SeaHasher)>(f) };
    // SAFETY: the caller must ensure that the pointer is valid and points to a value of type C
    let value = unsafe { ptr.deref::<C>() };
    trace!(
        "Hashing component value: {:?} into {:?}",
        value,
        core::any::type_name::<C>()
    );
    f(value, hasher);
}

fn default_inner_hash_fn<C: core::hash::Hash>(value: &C, hasher: &mut seahash::SeaHasher) {
    value.hash(hasher)
}

impl<C: Debug> ComponentRegistration<'_, C> {
    /// Add a hash function for this component using the `core::hash::Hash` trait implementation.
    ///
    /// This will be used to compute a checksum for determinism checks.
    pub fn add_default_hash(self) -> Self
    where
        C: core::hash::Hash + Component,
    {
        self.app
            .world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                let confirmed_component_id = world.register_component::<C>();
                let component_id = world.register_component::<C>();

                let kind = ComponentKind::of::<C>();
                registry
                    .component_metadata_map
                    .entry(kind)
                    .or_insert_with(|| ComponentMetadata {
                        confirmed_component_id,
                        component_id,
                        replication: None,
                        serialization: None,
                        delta: None,
                        deterministic: None,
                    })
                    .deterministic = Some(DeterministicFns::new::<C>(default_inner_hash_fn::<C>));
            });
        self
    }

    /// Add a hash function for this component using the `core::hash::Hash` trait implementation.
    ///
    /// This will be used to compute a checksum for determinism checks.
    pub fn add_custom_hash(self, f: fn(&C, &mut seahash::SeaHasher)) -> Self
    where
        C: Component,
    {
        self.app
            .world_mut()
            .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
                let confirmed_component_id = world.register_component::<C>();
                let component_id = world.register_component::<C>();
                let kind = ComponentKind::of::<C>();
                registry
                    .component_metadata_map
                    .entry(kind)
                    .or_insert_with(|| ComponentMetadata {
                        confirmed_component_id,
                        component_id,
                        replication: None,
                        serialization: None,
                        delta: None,
                        deterministic: None,
                    })
                    .deterministic = Some(DeterministicFns::new(f));
            });
        self
    }
}
