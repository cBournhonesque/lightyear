use crate::error::ReplicationError;
use crate::prelude::ComponentRegistry;
use crate::registry::ComponentKind;
use alloc::vec::Vec;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::Resource;
use bevy_ptr::Ptr;
use core::ops::Range;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::writer::Writer;

/// Single continuous buffer that stores serialized data for messages.
///
/// See [`Updates`](super::actions::Updates) and
/// [`MutateMessage`](super::updates::MutateMessage).
#[derive(Resource, Deref, DerefMut, Default)]
pub(crate) struct SerializedData(Vec<u8>);

pub(crate) type ByteRange = Range<usize>;

/// Custom serialization for replication messages.
pub(crate) trait MessageWrite {
    /// Writes data for replication messages and returns a range that points to it.
    fn write(&self, serialized: &mut SerializedData) -> Result<Range<usize>, ReplicationError>;

    /// Like [`Self::write`], but returns the value from the range if it's [`Some`].
    fn write_cached(
        &self,
        serialized: &mut SerializedData,
        cached_range: &mut Option<Range<usize>>,
    ) -> Result<Range<usize>, ReplicationError> {
        if let Some(range) = cached_range.clone() {
            return Ok(range);
        }

        let range = self.write(serialized)?;
        *cached_range = Some(range.clone());

        Ok(range)
    }
}

pub(crate) struct WritableComponent<'a> {
    pub(crate) ptr: Ptr<'a>,
    pub(crate) kind: &'a ComponentKind,
    pub(crate) registry: &'a ComponentRegistry,
}

impl MessageWrite for WritableComponent<'_> {
    fn write(&self, serialized: &mut SerializedData) -> Result<Range<usize>, ReplicationError> {
        let start = serialized.len();

        // TODO: provide a function that serializes without entity mapping
        let mut sender_map = SendEntityMap::default();
        self.registry.erased_serialize(
            self.ptr,
            &mut Writer::from(&serialized.0),
            *self.kind,
            &mut sender_map,
        )?;

        let end = serialized.len();

        Ok(start..end)
    }
}

pub(crate) struct EntityMapping {
    pub(crate) entity: Entity,
    pub(crate) hash: u64,
}

impl MessageWrite for EntityMapping {
    fn write(&self, serialized: &mut SerializedData) -> Result<Range<usize>, ReplicationError> {
        let start = serialized.len();

        self.entity.write(serialized)?;
        serialized.extend(self.hash.to_le_bytes()); // Use fixint encoding because it's more efficient for hashes.

        let end = serialized.len();

        Ok(start..end)
    }
}

impl<T: ToBytes> MessageWrite for T {
    fn write(&self, serialized: &mut SerializedData) -> Result<Range<usize>, ReplicationError> {
        let start = serialized.len();
        self.to_bytes(&mut serialized.0)?;
        let end = serialized.len();
        Ok(start..end)
    }
}
