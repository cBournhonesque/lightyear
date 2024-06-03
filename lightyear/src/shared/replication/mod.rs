//! Module to handle replicating entities and components from server to client
use std::fmt::Debug;
use std::hash::Hash;
use std::io::Seek;

use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::{Entity, Resource};
use bevy::utils::HashSet;
use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};

use crate::connection::id::ClientId;
use crate::packet::message::MessageId;
use crate::prelude::Tick;
use crate::protocol::registry::NetId;
use crate::protocol::EventContext;
use crate::serialize::varint::{varint_len, VarIntReadExt, VarIntWriteExt};
use crate::serialize::writer::Writer;
use crate::serialize::{RawData, SerializationError, ToBytes};
use crate::shared::events::connection::{
    ClearEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    IterEntityDespawnEvent, IterEntitySpawnEvent,
};
use crate::shared::replication::components::ReplicationGroupId;

pub mod components;

pub mod delta;
pub mod entity_map;
pub(crate) mod hierarchy;
pub mod network_target;
pub(crate) mod plugin;
pub(crate) mod receive;
pub(crate) mod resources;
pub(crate) mod send;
pub(crate) mod systems;

/// Serialize Entity as two varints for the index and generation (because they will probably be low).
/// Revisit this when relations comes out
///
/// TODO: optimize for the case where generation == 1, which should be most cases
impl ToBytes for Entity {
    fn len(&self) -> usize {
        varint_len(self.index() as u64) + varint_len(self.generation() as u64)
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_varint(self.index() as u64)?;
        buffer.write_varint(self.generation() as u64)?;
        Ok(())
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let index = buffer.read_varint()? as u32;
        let generation = buffer.read_varint()? as u32;
        Ok(Entity::new(index, generation))
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityActions {
    pub(crate) spawn: SpawnAction,
    // TODO: maybe do HashMap<NetId, RawData>? for example for ShouldReuseTarget
    pub(crate) insert: Vec<RawData>,
    #[bitcode(with_serde)]
    // TODO: use a ComponentNetId instead of NetId?
    pub(crate) remove: HashSet<NetId>,
    pub(crate) updates: Vec<RawData>,
}

impl ToBytes for EntityActions {
    fn len(&self) -> usize {
        self.spawn.len() + todo!()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        self.spawn.to_bytes(buffer)?;

        todo!()
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        todo!()
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
    fn len(&self) -> usize {
        match &self {
            SpawnAction::None => 1,
            SpawnAction::Spawn => 1,
            SpawnAction::Despawn => 1,
            SpawnAction::Reuse(entity) => 1 + entity.len(),
        }
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
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

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
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
            remove: HashSet::new(),
            updates: Vec::new(),
        }
    }
}

// TODO: 99% of the time the ReplicationGroup is the same as the Entity in the hashmap, and there's only 1 entity
//  have an optimization for that
/// All the entity actions (Spawn/despawn/inserts/removals) for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityActionsMessage {
    sequence_id: MessageId,
    group_id: ReplicationGroupId,
    #[bitcode(with_serde)]
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions)>,
}

impl ToBytes for EntityActionsMessage {
    fn len(&self) -> usize {
        self.sequence_id.len()
            + self.group_id.len()
            + self.actions.len()
                * (std::mem::size_of::<Entity>() + std::mem::size_of::<EntityActions>())
    }

    fn to_bytes<T: byteorder::WriteBytesExt>(
        &self,
        buffer: &mut T,
    ) -> Result<(), SerializationError> {
        self.sequence_id.to_bytes(buffer)?;
        self.group_id.to_bytes(buffer)?;
        for (entity, actions) in &self.actions {
            entity.to_bytes(buffer)?;
            actions.to_bytes(buffer)?;
        }
        Ok(())
    }

    fn from_bytes<T: byteorder::ReadBytesExt + Seek>(
        buffer: &mut T,
    ) -> Result<Self, SerializationError> {
        let sequence_id = MessageId::from_bytes(buffer)?;
        let group_id = ReplicationGroupId::from_bytes(buffer)?;
        let mut actions = Vec::new();
        while buffer.has_remaining() {
            let entity = Entity::from_bytes(buffer)?;
            let actions = EntityActions::from_bytes(buffer)?;
            actions.push((entity, actions));
        }
        Ok(Self {
            sequence_id,
            group_id,
            actions,
        })
    }
}

/// All the component updates for the entities of a given [`ReplicationGroup`](crate::prelude::ReplicationGroup)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityUpdatesMessage {
    pub(crate) group_id: ReplicationGroupId,
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    /// Updates containing the full component data
    #[bitcode(with_serde)]
    pub(crate) updates: Vec<(Entity, Vec<RawData>)>,
    // /// Updates containing diffs with a previous value
    // #[bitcode(with_serde)]
    // diff_updates: Vec<(Entity, Vec<RawData>)>,
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
    type Error: std::error::Error;
    type ReplicateCache;
    fn writer(&mut self) -> &mut Writer;

    /// Return the list of clients that connected to the server since we last sent any replication messages
    /// (this is used to send the initial state of the world to new clients)
    fn new_connected_clients(&self) -> Vec<ClientId>;

    /// Get the replication cache
    fn replication_cache(&mut self) -> &mut Self::ReplicateCache;

    /// Any operation that needs to happen before we can send the replication messages
    /// (for example collecting the individual single component updates into a single message,
    ///
    /// Similarly, we want to collect all ComponentInsert and ComponentRemove into a single message.
    /// Why? Because if we create separate message for each ComponentInsert (for example when the entity gets spawned)
    /// Then those 2 component inserts might be stored in different packets, and arrive at different times because of jitter
    ///
    /// But the receiving systems might expect both components to be present at the same time.
    fn buffer_replication_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
    ) -> Result<(), Self::Error>;

    /// Do some regular cleanup on the internals of replication
    /// - account for tick wrapping by resetting some internal ticks for each replication group
    fn cleanup(&mut self, tick: Tick);
}
