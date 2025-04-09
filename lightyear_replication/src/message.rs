use crate::components::ReplicationGroupId;
use crate::registry::ComponentNetId;
use bevy::ecs::entity::EntityHash;
use bevy::platform_support::collections::HashMap;
use bevy::prelude::Entity;
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::packet::message::MessageId;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Default channel to replicate entity actions.
/// This is an Unordered Reliable channel.
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
pub struct ActionsChannel;

/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
pub struct UpdatesChannel;

/// All the entity actions (Spawn/despawn/inserts/removals) for a single entity
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
        self.spawn.bytes_len() + self.insert.bytes_len() + self.remove.bytes_len() + self.updates.bytes_len()
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
    Spawn,
    Despawn,
    // the u64 is the entity's bits (we cannot use Entity directly because it doesn't implement Encode/Decode)
    Reuse(Entity),
}

impl ToBytes for SpawnAction {
    fn bytes_len(&self) -> usize {
        match &self {
            SpawnAction::None => 1,
            SpawnAction::Spawn => 1,
            SpawnAction::Despawn => 1,
            SpawnAction::Reuse(entity) => 1 + entity.bytes_len(),
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match &self {
            SpawnAction::None => buffer.write_u8(0)?,
            SpawnAction::Spawn => buffer.write_u8(1)?,
            SpawnAction::Despawn => buffer.write_u8(2)?,
            SpawnAction::Reuse(entity) => {
                buffer.write_u8(3)?;
                entity.to_bytes(buffer)?;
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
            1 => Ok(SpawnAction::Spawn),
            2 => Ok(SpawnAction::Despawn),
            3 => Ok(SpawnAction::Reuse(Entity::from_bytes(buffer)?)),
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

#[derive(Clone, PartialEq, Debug)]
pub struct SendEntityActionsMessage {
    pub(crate) sequence_id: MessageId,
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) actions: HashMap<Entity, EntityActions, EntityHash>,
}


impl ToBytes for SendEntityActionsMessage {
    fn bytes_len(&self) -> usize {
        self.sequence_id.bytes_len() + self.group_id.bytes_len() + self.actions.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.sequence_id.to_bytes(buffer)?;
        self.group_id.to_bytes(buffer)?;
        self.actions.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        Ok(Self {
            sequence_id: MessageId::from_bytes(buffer)?,
            group_id: ReplicationGroupId::from_bytes(buffer)?,
            actions: HashMap::<Entity, EntityActions, EntityHash>::from_bytes(buffer)?,
        })
    }
}

// TODO: 99% of the time the ReplicationGroup is the same as the Entity in the hashmap, and there's only 1 entity
//  have an optimization for that
/// All the entity actions (Spawn/despawn/inserts/removals) for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Clone, PartialEq, Debug)]
pub struct ActionsMessage {
    pub(crate) sequence_id: MessageId,
    pub(crate) group_id: ReplicationGroupId,
    // TODO: for better compression, we should use columnar storage
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions)>,
}

impl ToBytes for ActionsMessage {
    fn bytes_len(&self) -> usize {
        self.sequence_id.bytes_len() + self.group_id.bytes_len() + self.actions.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.sequence_id.to_bytes(buffer)?;
        self.group_id.to_bytes(buffer)?;
        self.actions.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        Ok(Self {
            sequence_id: MessageId::from_bytes(buffer)?,
            group_id: ReplicationGroupId::from_bytes(buffer)?,
            actions: Vec::<(Entity, EntityActions)>::from_bytes(buffer)?,
        })
    }
}

/// Same as EntityUpdatesMessage, but avoids having to convert a hashmap into a vec
#[derive(Clone, PartialEq, Debug)]
pub struct UpdatesSendMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    pub(crate) last_action_tick: Option<Tick>,
    /// Updates containing the full component data
    pub(crate) updates: HashMap<Entity, Vec<Bytes>, EntityHash>,
    // /// Updates containing diffs with a previous value
    // #[bitcode(with_serde)]
    // diff_updates: Vec<(Entity, Vec<RawData>)>,
}

impl ToBytes for UpdatesSendMessage {
    fn bytes_len(&self) -> usize {
        self.group_id.bytes_len() + self.last_action_tick.bytes_len() + self.updates.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.group_id.to_bytes(buffer)?;
        self.last_action_tick.to_bytes(buffer)?;
        self.updates.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self {
            group_id: ReplicationGroupId::from_bytes(buffer)?,
            last_action_tick: Option::<Tick>::from_bytes(buffer)?,
            updates: HashMap::<Entity, Vec<Bytes>, EntityHash>::from_bytes(buffer)?,
        })
    }
}

/// All the component updates for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Clone, PartialEq, Debug)]
pub struct UpdatesMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    pub(crate) last_action_tick: Option<Tick>,
    /// Updates containing the full component data
    pub(crate) updates: Vec<(Entity, Vec<Bytes>)>,
    // /// Updates containing diffs with a previous value
    // #[bitcode(with_serde)]
    // diff_updates: Vec<(Entity, Vec<RawData>)>,
}

impl ToBytes for UpdatesMessage {
    fn bytes_len(&self) -> usize {
        self.group_id.bytes_len() + self.last_action_tick.bytes_len() + self.updates.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.group_id.to_bytes(buffer)?;
        self.last_action_tick.to_bytes(buffer)?;
        self.updates.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self {
            group_id: ReplicationGroupId::from_bytes(buffer)?,
            last_action_tick: Option::<Tick>::from_bytes(buffer)?,
            updates: Vec::<(Entity, Vec<Bytes>)>::from_bytes(buffer)?,
        })
    }
}