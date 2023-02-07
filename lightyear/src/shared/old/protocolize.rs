use std::{any::TypeId, hash::Hash};

use lightyear_serde::{BitReader, BitWrite, Serde, SerdeErr};

use crate::{protocol::component_update::ComponentUpdate, DiffMask, NetEntityConverter};

use super::{
    replica_ref::{ReplicaDynMut, ReplicaDynRef},
    replicate::{Replicate, ReplicateSafe},
};

/// An Enum with a variant for every Component/Message that can be sent
/// between Client/Host
pub trait Protocolize: Clone + Sized + Sync + Send + 'static {
    type Kind: ProtocolKindType;

    /// Get name of variant
    fn name(&self) -> String;
    /// Get kind of Replicate type
    fn kind_of<R: ReplicateSafe<Self>>() -> Self::Kind;
    /// Get kind from a type_id
    fn type_to_kind(type_id: TypeId) -> Option<Self::Kind>;
    /// Read from a bit stream to create a new Replica
    fn read(
        reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<Self, SerdeErr>;
    /// Read from a bit stream to create a new Component Update
    fn read_create_update(reader: &mut BitReader) -> Result<ComponentUpdate<Self::Kind>, SerdeErr>;
    /// Get an immutable reference to the inner Component/Message as a
    /// Replicate trait object
    fn dyn_ref(&self) -> ReplicaDynRef<'_, Self>;
    /// Get an mutable reference to the inner Component/Message as a
    /// Replicate trait object
    fn dyn_mut(&mut self) -> ReplicaDynMut<'_, Self>;
    /// Cast to a Replicate impl
    fn cast<R: Replicate<Self>>(self) -> Option<R>;
    /// Cast to a typed immutable reference to the inner Component/Message
    fn cast_ref<R: ReplicateSafe<Self>>(&self) -> Option<&R>;
    /// Cast to a typed mutable reference to the inner Component/Message
    fn cast_mut<R: ReplicateSafe<Self>>(&mut self) -> Option<&mut R>;
    /// Extract an inner Replicate impl from the Protocolize into a
    /// ProtocolInserter impl
    fn extract_and_insert<N, X: ProtocolInserter<Self, N>>(&self, entity: &N, inserter: &mut X);
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
}

pub trait ProtocolKindType: Eq + Hash + Copy + Send + Sync + Serde {
    fn to_type_id(&self) -> TypeId;
    fn name(&self) -> String;
}

pub trait ProtocolInserter<P: Protocolize, N> {
    fn insert<R: ReplicateSafe<P>>(&mut self, entity: &N, component: R);
}
