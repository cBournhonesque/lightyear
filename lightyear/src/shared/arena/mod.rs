//! Provides a ArenaManager that wraps a slab allocator that can be used when
//! allocating a lot of small objects.
//!
//! This is best used when the allocations are needed in phases: a lot of small
//! objects need to be allocated, and then are suddenly not needed and can be
//! deallocated all at once.

use crate::prelude::MainSet;
use crate::serialize::reader::Reader;
use crate::serialize::varint::varint_len;
use crate::serialize::{SerializationError, ToBytes};
use bevy::ecs::entity::EntityHash;
use bevy::prelude::*;
use blink_alloc::SyncBlinkAlloc;
use byteorder::{ReadBytesExt, WriteBytesExt};

pub type ArenaEntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash, &'static SyncBlinkAlloc>;
pub type ArenaVec<T> = allocator_api2::vec::Vec<T, &'static SyncBlinkAlloc>;

impl<M: ToBytes> ToBytes for ArenaVec<M> {
    fn len(&self) -> usize {
        varint_len(self.len() as u64) + self.iter().map(ToBytes::len).sum::<usize>()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_u64::<byteorder::NetworkEndian>(self.len() as u64)?;
        self.iter().try_for_each(|item| item.to_bytes(buffer))?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let len = buffer.read_u64::<byteorder::NetworkEndian>()? as usize;
        // TODO: if we know the MIN_LEN we can preallocate

        let mut vec = ArenaVec::with_capacity_in(len, ArenaManager::fake_alloc());
        for _ in 0..len {
            vec.push(M::from_bytes(buffer)?);
        }
        // NOTE: we don't need the allocator anymore upon deserializing! just use the global alloc
        Ok(vec)
    }
}

#[derive(Default)]
pub struct ArenaPlugin;

impl ArenaPlugin {
    fn reset(mut arena: ResMut<ArenaManager>) {
        arena.allocator.reset();
    }
}

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArenaManager>();

        app.add_systems(
            PostUpdate,
            Self::reset
                .in_set(MainSet::Send)
                .after(MainSet::SendPackets),
        );
    }
}

#[derive(Resource)]
pub struct ArenaManager {
    pub(crate) allocator: SyncBlinkAlloc,
}

impl ArenaManager {
    pub(crate) fn get(&self) -> &'static SyncBlinkAlloc {
        unsafe { std::mem::transmute(&self.allocator) }
    }

    /// In some situations we might need a fake allocator if we know we are not going to use it
    /// at all
    pub(crate) fn fake_alloc() -> &'static SyncBlinkAlloc {
        let data = [0; 64];
        unsafe { std::mem::transmute(&data) }
    }
}

impl Default for ArenaManager {
    fn default() -> Self {
        Self {
            allocator: SyncBlinkAlloc::new(),
        }
    }
}
