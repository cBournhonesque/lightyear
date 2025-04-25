//! Module to handle replicating entities and components from server to client
#[cfg(not(feature = "std"))]
use alloc::{vec::Vec};
use bevy::ecs::entity::EntityHash;
use core::fmt::Debug;
use core::hash::Hash;

use bevy::platform::collections::HashMap;
use bevy::prelude::{Entity, Resource};
use bytes::Bytes;

use crate::connection::id::ClientId;
use crate::packet::message::MessageId;
use crate::prelude::Tick;
use crate::protocol::component::ComponentNetId;
use crate::protocol::EventContext;
use crate::serialize::reader::{ReadInteger, ReadVarInt, Reader};
use crate::serialize::varint::{varint_len};
use crate::serialize::writer::{WriteInteger, Writer};
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::events::connection::{
    ClearEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    IterEntityDespawnEvent, IterEntitySpawnEvent,
};
use crate::shared::replication::components::ReplicationGroupId;

pub(crate) mod archetypes;
pub mod components;

pub(crate) mod authority;
pub mod delta;
pub mod entity_map;
pub mod error;
pub(crate) mod hierarchy;
pub mod network_target;
pub(crate) mod plugin;
pub(crate) mod prespawn;
pub(crate) mod receive;
pub(crate) mod resources;
pub(crate) mod send;
pub(crate) mod systems;
/// Serialize Entity as two varints for the index and generation (because they will probably be low).
/// Revisit this when relations comes out
///
/// TODO: optimize for the case where generation == 1, which should be most cases
impl ToBytes for Entity {
    fn bytes_len(&self) -> usize {
        varint_len(self.index() as u64) + 4
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_varint(self.index() as u64)?;
        buffer.write_u32(self.generation())?;
        // buffer.write_varint(self.generation() as u64)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let index = buffer.read_varint()?;

        // TODO: investigate why it doesn't work with varint?
        // NOTE: not that useful now that we use a high bit to symbolize 'is_masked'
        // let generation = buffer.read_varint()?;
        let generation = buffer.read_u32()? as u64;
        let bits = generation << 32 | index;
        Ok(Entity::from_bits(bits))
    }
}

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
    sequence_id: MessageId,
    group_id: ReplicationGroupId,
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
pub struct EntityActionsMessage {
    sequence_id: MessageId,
    group_id: ReplicationGroupId,
    // TODO: for better compression, we should use columnar storage
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions)>,
}

impl ToBytes for EntityActionsMessage {
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
pub struct SendEntityUpdatesMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    /// Updates containing the full component data
    pub(crate) updates: HashMap<Entity, Vec<Bytes>, EntityHash>,
    // /// Updates containing diffs with a previous value
    // #[bitcode(with_serde)]
    // diff_updates: Vec<(Entity, Vec<RawData>)>,
}

impl ToBytes for SendEntityUpdatesMessage {
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
pub struct EntityUpdatesMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    /// Updates containing the full component data
    pub(crate) updates: Vec<(Entity, Vec<Bytes>)>,
    // /// Updates containing diffs with a previous value
    // #[bitcode(with_serde)]
    // diff_updates: Vec<(Entity, Vec<RawData>)>,
}

impl ToBytes for EntityUpdatesMessage {
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

/// Trait for a service that participates in replication.
pub(crate) trait ReplicationPeer: Resource {
    type Events: IterComponentInsertEvent<Self::EventContext>
        + IterComponentRemoveEvent<Self::EventContext>
        + IterComponentUpdateEvent<Self::EventContext>
        + IterEntitySpawnEvent<Self::EventContext>
        + IterEntityDespawnEvent<Self::EventContext>
        + ClearEvents;
    /// Type of the context associated with the events emitted/received by this replication peer
    type EventContext: EventContext;

    /// Marker to identify the type of the ReplicationSet component
    /// This is mostly relevant in the unified mode, where a ReplicationSet can be added several times
    /// (in the client and the server replication plugins)
    type SetMarker: Debug + Hash + Send + Sync + Eq + Clone;
}

/// Trait for a service that receives replication messages.
pub(crate) trait ReplicationReceive: Resource + ReplicationPeer {
    /// The received events buffer
    fn events(&mut self) -> &mut Self::Events;

    /// Do some regular cleanup on the internals of replication
    /// - account for tick wrapping by resetting some internal ticks for each replication group
    fn cleanup(&mut self, tick: Tick);
}

#[doc(hidden)]
/// Trait for any service that can send replication messages to the remote.
/// (this trait is used to easily enable both client to server and server to client replication)
///
/// The trait is made public because it is needed in the macros
pub(crate) trait ReplicationSend: Resource + ReplicationPeer {
    type Error: core::error::Error;
    fn writer(&mut self) -> &mut Writer;

    /// Return the list of clients that connected to the server since we last sent any replication messages
    /// (this is used to send the initial state of the world to new clients)
    fn new_connected_clients(&self) -> Vec<ClientId>;

    /// Do some regular cleanup on the internals of replication
    /// - account for tick wrapping by resetting some internal ticks for each replication group
    fn cleanup(&mut self, tick: Tick);
}
