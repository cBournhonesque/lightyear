use crate::registry::ComponentNetId;
use bevy_ecs::{
    entity::{Entity, EntityHash},
    error::Result,
    event::Event,
};
use bevy_platform::collections::HashMap;
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_core::time::PositiveTickDelta;
use lightyear_serde::{reader::{ReadInteger, Reader}, varint::varint_len};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::packet::packet_builder::MAX_PACKET_SIZE;

use crate::prelude::ReplicationGroupId;
use alloc::vec::Vec;
use std::marker::PhantomData;

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
pub const MAX_MESSAGE_SIZE: usize = MAX_PACKET_SIZE - 100;


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
        predicted: bool,
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
                predicted,
                interpolated,
                prespawn,
            } => {
                if *predicted && *interpolated {
                    return Err(SerializationError::InvalidValue);
                }
                // Use bits to store the flags:
                // 0: predicted
                // 1: interpolated
                // 2: prespawn.is_some()
                let mut flags = 0u8;
                if *predicted {
                    flags |= 1 << 0;
                }
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
                let predicted = (flags & (1 << 0)) != 0;
                let interpolated = (flags & (1 << 1)) != 0;
                let has_prespawn = (flags & (1 << 2)) != 0;

                let prespawn = if has_prespawn {
                    Some(buffer.read_u64()?)
                } else {
                    None
                };
                Ok(SpawnAction::Spawn {
                    predicted,
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
    pub(crate) entity_count: usize,
    pub(crate) num_bytes: usize,
    pub(crate) writer: &'a mut Writer,
    marker: PhantomData<T>,
}

impl<'a, T: ToBytes> MessageBuilder<'a, T> {
    pub(crate) fn new(sequence_id: MessageId, group_id: ReplicationGroupId, writer: &'a mut Writer) -> Result<Self, SerializationError> {
        sequence_id.to_bytes(writer)?;
        group_id.to_bytes(writer)?;
        // the extra 1 is for the entity count
        let num_bytes = ToBytes::bytes_len(&sequence_id) + ToBytes::bytes_len(&group_id) + 1;
        Ok(Self {
            group_id,
            entity_count: 0,
            num_bytes,
            writer,
            marker: PhantomData,
        })
    }

    /// Try to add another piece of data to the message.
    ///
    /// If the message is too big, return false and do not add the data to the message.
    pub(crate) fn add_data(&mut self, entity: Entity, data: T) -> Result<bool, SerializationError> {
        let entity_bytes = entity.bytes_len();
        let data_bytes = data.bytes_len();
        let total_bytes = entity_bytes + data_bytes;
        if self.entity_count > 0 && self.num_bytes + total_bytes > MAX_MESSAGE_SIZE {
            return Ok(false);
        }
        self.num_bytes += total_bytes;
        self.entity_count += 1;
        entity.to_bytes(self.writer)?;
        data.to_bytes(self.writer)?;
        Ok(true);
    }

    /// Return the bytes corresponding to the `ActionsMessage`
    pub(crate) fn build(self) -> Result<Bytes, SerializationError> {
        self.writer.write_varint(self.entity_count as u64)?;
        return self.writer.split();
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
    // TODO: for better compression we should have a Vec<Entity> and a Vec<EntityActions>
    pub(crate) data: Bytes,
}

impl ToBytes for ActionsMessage {
    fn bytes_len(&self) -> usize {
        unimplemented!("Use SendEntityActionsMessage on the read side");
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        unimplemented!("Use SendEntityActionsMessage on the read side");
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        let sequence_id = MessageId::from_bytes(buffer)?;
        let group_id = ReplicationGroupId::from_bytes(buffer)?;
        let entity_count = buffer.read_varint()? as usize;
        let data = Bytes::from_bytes(buffer)?;
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

impl IntoIterator for ActionsMessage {
    type Item = (Entity, EntityActions);
    type IntoIter = IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        ActionsMessageIter {
            reader: Reader::from(self.data),
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
    /// Updates containing the full component data, equivalent to Vec<(Entity, Vec<Bytes>)>
    pub(crate) data: Bytes,
}

impl ToBytes for UpdatesMessage {
    fn bytes_len(&self) -> usize {
        unimplemented!("This is only used on the read side");
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        unimplemented!("This is only used on the read side");
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self {
            group_id: ReplicationGroupId::from_bytes(buffer)?,
            last_action_tick: Option::<Tick>::from_bytes(buffer)?,
            data: Bytes::from_bytes(buffer)?,
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

impl IntoIterator for UpdatesMessage {
    type Item = (Entity, Vec<Bytes>);
    type IntoIter = IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        UpdatesMessageIter {
            reader: Reader::from(self.data),
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
