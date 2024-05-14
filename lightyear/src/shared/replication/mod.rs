//! Module to handle replicating entities and components from server to client
use std::fmt::Debug;
use std::hash::Hash;

use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::{Component, Entity, Resource};
use bevy::reflect::Map;
use bevy::utils::HashSet;
use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};
use network_target::NetworkTarget;

use crate::channel::builder::Channel;
use crate::connection::id::ClientId;
use crate::packet::message::MessageId;
use crate::prelude::{ReplicationGroup, Tick};
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::protocol::registry::NetId;
use crate::protocol::EventContext;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::RawData;
use crate::shared::events::connection::{
    ClearEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    IterEntityDespawnEvent, IterEntitySpawnEvent,
};
use crate::shared::replication::components::{ReplicationGroupId, ReplicationTarget};
use crate::shared::replication::systems::ReplicateCache;

pub mod components;

mod commands;
pub mod entity_map;
pub(crate) mod hierarchy;
pub mod network_target;
pub(crate) mod plugin;
pub(crate) mod receive;
pub(crate) mod resources;
pub(crate) mod send;
pub(crate) mod systems;

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

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub(crate) enum SpawnAction {
    None,
    Spawn,
    Despawn,
    // the u64 is the entity's bits (we cannot use Entity directly because it doesn't implement Encode/Decode)
    Reuse(u64),
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
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityActionMessage {
    sequence_id: MessageId,
    #[bitcode(with_serde)]
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityUpdatesMessage {
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    #[bitcode(with_serde)]
    pub(crate) updates: Vec<(Entity, Vec<RawData>)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub enum ReplicationMessageData {
    /// All the entity actions (Spawn/despawn/inserts/removals) for a given group
    Actions(EntityActionMessage),
    /// All the entity updates for a given group
    Updates(EntityUpdatesMessage),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct ReplicationMessage {
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) data: ReplicationMessageData,
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
    fn writer(&mut self) -> &mut BitcodeWriter;

    /// Return the list of clients that connected to the server since we last sent any replication messages
    /// (this is used to send the initial state of the world to new clients)
    fn new_connected_clients(&self) -> Vec<ClientId>;

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()>;

    #[allow(clippy::too_many_arguments)]
    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        component_registry: &ComponentRegistry,
        replication_target: &ReplicationTarget,
        prediction_target: Option<&NetworkTarget>,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()>;

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: ComponentNetId,
        group: &ReplicationGroup,
        target: NetworkTarget,
    ) -> Result<()>;

    #[allow(clippy::too_many_arguments)]
    fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        group: &ReplicationGroup,
        target: NetworkTarget,
        // bevy_tick when the component changes
        component_change_tick: BevyTick,
        // bevy_tick for the current system run
        system_current_tick: BevyTick,
    ) -> Result<()>;

    /// Any operation that needs to happen before we can send the replication messages
    /// (for example collecting the individual single component updates into a single message,
    ///
    /// Similarly, we want to collect all ComponentInsert and ComponentRemove into a single message.
    /// Why? Because if we create separate message for each ComponentInsert (for example when the entity gets spawned)
    /// Then those 2 component inserts might be stored in different packets, and arrive at different times because of jitter
    ///
    /// But the receiving systems might expect both components to be present at the same time.
    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()>;

    fn get_mut_replicate_cache(&mut self) -> &mut EntityHashMap<ReplicateCache>;

    /// Do some regular cleanup on the internals of replication
    /// - account for tick wrapping by resetting some internal ticks for each replication group
    fn cleanup(&mut self, tick: Tick);
}
