use bevy_ecs::entity::Entity;
use lightyear_serde::{BitReader, BitWrite, SerdeErr};

use crate::shared::messages::named::Named;
use crate::shared::types::ComponentId;
use crate::shared::{component::{
    component_update::ComponentUpdate,
    diff_mask::DiffMask,
    property_mutate::PropertyMutator,
    replica_ref::{ReplicaDynMut, ReplicaDynRef},
}, NetEntityConverter};

/// A map to hold all component types
pub struct Components;

impl Components {
    pub fn type_to_id<R: ReplicateSafe>() -> ComponentId {
        todo!()
    }
    pub fn id_to_name(id: &ComponentId) -> String {
        todo!()
    }
    pub fn box_to_id(boxed_component: &Box<dyn ReplicateSafe>) -> ComponentId {
        todo!()
    }
    pub fn cast<R: Replicate>(boxed_component: Box<dyn ReplicateSafe>) -> Option<R> {
        todo!()
    }
    pub fn cast_ref<R: ReplicateSafe>(boxed_component: &Box<dyn ReplicateSafe>) -> Option<&R> {
        todo!()
    }
    pub fn cast_mut<R: ReplicateSafe>(
        boxed_component: &mut Box<dyn ReplicateSafe>,
    ) -> Option<&mut R> {
        todo!()
    }
    pub fn read(
        bit_reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<Box<dyn ReplicateSafe>, SerdeErr> {
        todo!()
    }

    pub fn write(
        bit_writer: &mut dyn BitWrite,
        converter: &dyn NetEntityConverter,
        message: &Box<dyn ReplicateSafe>,
    ) {
        todo!()
    }
    pub fn read_create_update(bit_reader: &mut BitReader) -> Result<ComponentUpdate, SerdeErr> {
        todo!()
    }
}

/// A struct that implements Replicate is a Message/Component, or otherwise,
/// a container of Properties that can be scoped, tracked, and synced, with a
/// remote host
pub trait Replicate: ReplicateSafe + Clone {}

/// The part of Replicate which is object-safe
pub trait ReplicateSafe: ReplicateInner + Named {
    /// Gets the ComponentId of this type
    fn kind(&self) -> ComponentId;
    /// Gets the number of bytes of the Component's DiffMask
    fn diff_mask_size(&self) -> u8;
    /// Get an immutable reference to the inner Component as a Replicate trait object
    fn dyn_ref(&self) -> ReplicaDynRef<'_>;
    /// Get an mutable reference to the inner Component as a Replicate trait object
    fn dyn_mut(&mut self) -> ReplicaDynMut<'_>;
    /// Sets the current Replica to the state of another Replica of the
    /// same type
    fn mirror(&mut self, other: &dyn ReplicateSafe);
    /// Set the Message/Component's PropertyMutator, which keeps track
    /// of which Properties have been mutated, necessary to sync only the
    /// Properties that have changed with the client
    fn set_mutator(&mut self, mutator: &PropertyMutator);
    /// Writes data into an outgoing byte stream, sufficient to completely
    /// recreate the Message/Component on the client
    fn write(&self, bit_writer: &mut dyn BitWrite, converter: &dyn NetEntityConverter);
    /// Write data into an outgoing byte stream, sufficient only to update the
    /// mutated Properties of the Message/Component on the client
    fn write_update(
        &self,
        diff_mask: &DiffMask,
        bit_writer: &mut dyn BitWrite,
        converter: &dyn NetEntityConverter,
    );
    /// Reads data from an incoming packet, sufficient to sync the in-memory
    /// Component with it's replica on the Server
    fn read_apply_update(
        &mut self,
        converter: &dyn NetEntityConverter,
        update: ComponentUpdate,
    ) -> Result<(), SerdeErr>;
    /// Returns whether has any EntityProperties
    fn has_entity_properties(&self) -> bool;
    /// Returns a list of Entities contained within the Replica's properties
    fn entities(&self) -> Vec<Entity>;
}

cfg_if! {
    if #[cfg(feature = "bevy_support")]
    {
        // Require that Bevy Component to be implemented
        use bevy_ecs::component::{TableStorage, Component};

        pub trait ReplicateInner: Component<Storage = TableStorage> + Sync + Send + 'static {}

        impl<T> ReplicateInner for T
        where T: Component<Storage = TableStorage> + Sync + Send + 'static {
        }
    }
    else
    {
        pub trait ReplicateInner: Sync + Send + 'static {}

        impl<T> ReplicateInner for T
        where T: Sync + Send + 'static {
        }
    }
}
