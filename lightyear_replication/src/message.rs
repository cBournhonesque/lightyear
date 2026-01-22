use crate::registry::ComponentNetId;
use bevy_ecs::{entity::Entity, error::Result, event::Event};
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_core::time::PositiveTickDelta;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::packet::packet_builder::MAX_PACKET_SIZE;

use crate::prelude::{DEFAULT_GROUP, ReplicationGroupId};
use alloc::vec::Vec;
use core::marker::PhantomData;

/// Default channel to replicate entity actions.
/// This is an Unordered Reliable channel.
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
pub struct ActionsChannel;

/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
pub struct UpdatesChannel;

/// Default reliable channel to replicate metadata about the Sender or the connection
pub struct MetadataChannel;

/// Maximum size of a single replication message when sending to the default group
/// We take some margin as the length computation is not exact.
pub const MAX_MESSAGE_SIZE: usize = MAX_PACKET_SIZE - 50;

/// All the entity actions (Spawn/despawn/inserts/removals) for a single entity
#[doc(hidden)]
#[derive(Clone, PartialEq, Debug)]
pub struct EntityActions {
    pub(crate) spawn: SpawnAction,
    // TODO: maybe do HashMap<NetId, RawData>? for example for ShouldReuseTarget
    pub(crate) insert: Vec<Bytes>,
    pub(crate) remove: Vec<ComponentNetId>,
    pub(crate) updates: Vec<Bytes>,
}

impl ToBytes for EntityActions {
    fn bytes_len(&self) -> usize {
        self.spawn.bytes_len()
            + self.insert.bytes_len()
            + self.remove.bytes_len()
            + self.updates.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.spawn.to_bytes(buffer)?;
        self.insert.to_bytes(buffer)?;
        self.remove.to_bytes(buffer)?;
        self.updates.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self {
            spawn: SpawnAction::from_bytes(buffer)?,
            insert: Vec::from_bytes(buffer)?,
            remove: Vec::from_bytes(buffer)?,
            updates: Vec::from_bytes(buffer)?,
        })
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum SpawnAction {
    None,
    Spawn {
        // TODO: make it impossible to enable both predicted and interpolatd
        #[cfg(feature = "prediction")]
        predicted: bool,
        #[cfg(feature = "interpolation")]
        interpolated: bool,
        // If a PreSpawn hash is provided, instead of spawning an entity on the receiver we will try to match
        // with an entity that has the same PreSpawn hash
        prespawn: Option<u64>,
    },
    Despawn,
}

impl ToBytes for SpawnAction {
    fn bytes_len(&self) -> usize {
        match &self {
            SpawnAction::None => 1,
            SpawnAction::Despawn => 1,
            SpawnAction::Spawn { prespawn, .. } => 1 + prespawn.map_or(0, |p| 8),
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match &self {
            SpawnAction::None => buffer.write_u8(0)?,
            SpawnAction::Despawn => buffer.write_u8(1)?,
            SpawnAction::Spawn {
                #[cfg(feature = "prediction")]
                predicted,
                #[cfg(feature = "interpolation")]
                interpolated,
                prespawn,
            } => {
                #[cfg(all(feature = "prediction", feature = "interpolation"))]
                if *predicted && *interpolated {
                    return Err(SerializationError::InvalidValue);
                }
                // Use bits to store the flags:
                // 0: predicted
                // 1: interpolated
                // 2: prespawn.is_some()
                let mut flags = 0u8;
                #[cfg(feature = "prediction")]
                if *predicted {
                    flags |= 1 << 0;
                }
                #[cfg(feature = "interpolation")]
                if *interpolated {
                    flags |= 1 << 1;
                }
                if prespawn.is_some() {
                    flags |= 1 << 2;
                }

                // The spawn variant starts at tag 2
                buffer.write_u8(2 + flags)?;

                if let Some(prespawn) = prespawn {
                    buffer.write_u64(*prespawn)?;
                }
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        match buffer.read_u8()? {
            0 => Ok(SpawnAction::None),
            1 => Ok(SpawnAction::Despawn),
            // Spawn variants are in the range [2, 9]
            val @ 2..=9 => {
                let flags = val - 2;
                #[cfg(feature = "prediction")]
                let predicted = (flags & (1 << 0)) != 0;
                #[cfg(feature = "interpolation")]
                let interpolated = (flags & (1 << 1)) != 0;
                let has_prespawn = (flags & (1 << 2)) != 0;

                let prespawn = if has_prespawn {
                    Some(buffer.read_u64()?)
                } else {
                    None
                };
                Ok(SpawnAction::Spawn {
                    #[cfg(feature = "prediction")]
                    predicted,
                    #[cfg(feature = "interpolation")]
                    interpolated,
                    prespawn,
                })
            }
            _ => Err(SerializationError::InvalidPacketType),
        }
    }
}

impl Default for EntityActions {
    fn default() -> Self {
        Self {
            spawn: SpawnAction::None,
            insert: Vec::new(),
            remove: Vec::new(),
            updates: Vec::new(),
        }
    }
}

/// Helps us build replication messages while avoiding intermediary allocations.
///
/// Instead of storing an intermediate Vec or HashMap containing the serialized updates,
/// we serialize them directly into the final [`Writer`].
///
/// The data written is already serialized (for example `EntityActions` or `EntityUpdates`)
pub(crate) struct MessageBuilder<'a, T: ToBytes> {
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) entity_count: u16,
    // position in the buffer where `entity_count` is written
    pub(crate) entity_count_offset: usize,
    pub(crate) writer: &'a mut Writer,
    marker: PhantomData<T>,
}

impl<'a, T: ToBytes> MessageBuilder<'a, T> {
    pub(crate) fn new(
        group_id: ReplicationGroupId,
        writer: &'a mut Writer,
    ) -> Result<Self, SerializationError> {
        group_id.to_bytes(writer)?;
        // we write the number of entities now, and we will update it at the end
        let entity_count_offset = writer.len();
        writer.write_u16(0)?;
        Ok(Self {
            group_id,
            entity_count: 0,
            entity_count_offset,
            writer,
            marker: PhantomData,
        })
    }

    /// Check if we can add data to this message.
    ///
    /// If the group_id is 0, then we only add data if the message is smaller than the MTU
    pub(crate) fn can_add_data(&self, entity: Entity, data: &T) -> bool {
        let total_bytes = entity.bytes_len() + data.bytes_len();
        !(self.group_id == DEFAULT_GROUP.group_id(None)
            && self.entity_count > 0
            && self.writer.len() + total_bytes > MAX_MESSAGE_SIZE)
    }

    /// Try to add another piece of data to the message.
    ///
    /// If the message is too big, return false and do not add the data to the message.
    pub(crate) fn add_data(&mut self, entity: Entity, data: T) -> Result<bool, SerializationError> {
        self.entity_count += 1;
        entity.to_bytes(self.writer)?;
        data.to_bytes(self.writer)?;
        Ok(true)
    }

    /// Return the bytes corresponding to the `ActionsMessage`
    pub(crate) fn build(self) -> Result<Bytes, SerializationError> {
        // TODO: figure out how we can avoid using a u16 for this
        // backpatch the entity count
        let mut entity_count_slice =
            &mut self.writer.as_mut()[self.entity_count_offset..self.entity_count_offset + 2];
        entity_count_slice.write_u16(self.entity_count)?;
        Ok(self.writer.split())
    }
}

// TODO: 99% of the time the ReplicationGroup is the same as the Entity in the hashmap,
// and there's only 1 entity. have an optimization for that
/// All the entity actions (Spawn/despawn/inserts/removals) for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ActionsMessage {
    pub(crate) sequence_id: MessageId,
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) entity_count: usize,
    // Equivalent to Vec<(Entity, EntityActions)>
    // TODO: for better compression we should have a Vec<Entity> and a Vec<EntityActions>
    pub(crate) data: Bytes,
}

impl ToBytes for ActionsMessage {
    fn bytes_len(&self) -> usize {
        unimplemented!("Use SendEntityActionsMessage on the read side");
    }

    fn to_bytes(&self, _: &mut impl WriteInteger) -> Result<(), SerializationError> {
        unimplemented!("Use SendEntityActionsMessage on the read side");
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        let sequence_id = MessageId::from_bytes(buffer)?;
        let group_id = ReplicationGroupId::from_bytes(buffer)?;
        let entity_count = buffer.read_u16()? as usize;
        let data = buffer.split();
        Ok(Self {
            sequence_id,
            group_id,
            entity_count,
            data,
        })
    }
}

/// Iterator over the entities and [`EntityActions`] in an [`ActionsMessage`]
pub(crate) struct ActionsMessageIter {
    reader: Reader,
    index: usize,
    entity_count: usize,
}

impl Iterator for ActionsMessageIter {
    type Item = (Entity, EntityActions);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.entity_count {
            return None;
        }
        let entity = Entity::from_bytes(&mut self.reader).ok()?;
        let actions = EntityActions::from_bytes(&mut self.reader).ok()?;
        self.index += 1;
        Some((entity, actions))
    }
}

impl IntoIterator for &ActionsMessage {
    type Item = (Entity, EntityActions);
    type IntoIter = ActionsMessageIter;

    fn into_iter(self) -> Self::IntoIter {
        ActionsMessageIter {
            reader: Reader::from(self.data.clone()),
            index: 0,
            entity_count: self.entity_count,
        }
    }
}

/// All the component updates for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct UpdatesMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    pub(crate) last_action_tick: Option<Tick>,
    pub(crate) entity_count: usize,
    /// Updates containing the full component data, equivalent to `Vec<(Entity, Vec<Bytes>)>`
    pub(crate) data: Bytes,
}

impl ToBytes for UpdatesMessage {
    fn bytes_len(&self) -> usize {
        unimplemented!("This is only used on the read side");
    }

    fn to_bytes(&self, _: &mut impl WriteInteger) -> Result<(), SerializationError> {
        unimplemented!("This is only used on the read side");
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        // read in the order that the data was written
        let last_action_tick = Option::<Tick>::from_bytes(buffer)?;
        let group_id = ReplicationGroupId::from_bytes(buffer)?;
        let entity_count = buffer.read_u16()? as usize;
        let data = buffer.split();
        Ok(Self {
            group_id,
            last_action_tick,
            entity_count,
            data,
        })
    }
}

/// Iterator over the entities and updates in an [`UpdatesMessage`]
pub(crate) struct UpdatesMessageIter {
    reader: Reader,
    index: usize,
    entity_count: usize,
}

impl Iterator for UpdatesMessageIter {
    type Item = (Entity, Vec<Bytes>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.entity_count {
            return None;
        }
        let entity = Entity::from_bytes(&mut self.reader).ok()?;
        let updates = Vec::from_bytes(&mut self.reader).ok()?;
        self.index += 1;
        Some((entity, updates))
    }
}

impl IntoIterator for &UpdatesMessage {
    type Item = (Entity, Vec<Bytes>);
    type IntoIter = UpdatesMessageIter;

    fn into_iter(self) -> Self::IntoIter {
        UpdatesMessageIter {
            reader: Reader::from(self.data.clone()),
            index: 0,
            entity_count: self.entity_count,
        }
    }
}

#[derive(Event, Debug)]
pub struct SenderMetadata {
    pub send_interval: PositiveTickDelta,
    pub sender_entity: Entity,
}

impl ToBytes for SenderMetadata {
    fn bytes_len(&self) -> usize {
        self.send_interval.bytes_len() + self.sender_entity.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.send_interval.to_bytes(buffer)?;
        self.sender_entity.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let send_interval = PositiveTickDelta::from_bytes(buffer)?;
        let sender_entity = Entity::from_bytes(buffer)?;
        Ok(Self {
            send_interval,
            sender_entity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageBuilder, UpdatesMessage};
    use crate::error::ReplicationError;
    use crate::prelude::ReplicationGroupId;
    use alloc::{vec, vec::Vec};
    use bevy_ecs::entity::Entity;
    use bytes::Bytes;
    use lightyear_core::tick::Tick;
    use lightyear_serde::ToBytes;
    use lightyear_serde::reader::Reader;
    use lightyear_serde::writer::Writer;
    use test_log::test;

    #[test]
    fn test_updates_message_serde() -> Result<(), ReplicationError> {
        let mut writer = Writer::with_capacity(100);

        let tick = Some(Tick(1));
        let group_id = ReplicationGroupId(0);
        tick.to_bytes(&mut writer)?;
        let mut builder = MessageBuilder::<Vec<Bytes>>::new(ReplicationGroupId(0), &mut writer)?;
        let entity = Entity::from_raw_u32(10).unwrap();
        let updates = vec![Bytes::from_static(&[2; 3])];
        builder.add_data(entity, updates.clone())?;
        let ser = builder.build()?;

        let mut reader = Reader::from(ser);
        let message = UpdatesMessage::from_bytes(&mut reader)?;
        assert_eq!(message.last_action_tick, tick);
        assert_eq!(message.group_id, group_id);
        assert_eq!(message.entity_count, 1);
        for (entity_serde, updates_serde) in message.into_iter() {
            assert_eq!(entity_serde, entity);
            assert_eq!(updates_serde, updates);
        }
        Ok(())
    }
}
