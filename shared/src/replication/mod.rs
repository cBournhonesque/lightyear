// for client: map from client_entity to server_entity

// bevy systems to add/update/remove components and entities
// (potentially let users choose which channel to use for these?)

use anyhow::Result;
use bevy::prelude::{Component, Entity, Resource};
use bitcode::__private::Serialize;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use lightyear_derive::ChannelInternal;

use crate::netcode::ClientId;
use crate::{
    BitSerializable, Channel, ChannelKind, ComponentProtocol, ComponentProtocolKind, Protocol,
    ReadBuffer, WriteBuffer,
};

mod entity_map;
pub mod manager;

/// Default channel to replicate entity updates reliably
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
#[derive(ChannelInternal)]
pub struct DefaultReliableChannel;

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated.
#[derive(Component, Clone, Copy)]
pub struct Replicate {
    // TODO: be able to specify channel separately for entiy spawn/despawn and component insertion/removal?
    /// Optional: the channel to use for replicating entity updates.
    // pub channel: Option<ChannelKind>,

    /// The channel to use for replication (indicated by the generic type)
    // TODO: distinguish between replicating actions (inserts/etc.) vs updates
    pub channel: ChannelKind,
    pub target: ReplicationTarget,
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates
    //  should we make this configurable?
}

impl Replicate {
    pub fn with_channel<C: Channel>() -> Self {
        Self {
            channel: ChannelKind::of::<C>(),
            ..Default::default()
        }
    }
}

impl Default for Replicate {
    fn default() -> Self {
        Self {
            channel: ChannelKind::of::<DefaultReliableChannel>(),
            target: ReplicationTarget::default(),
        }
    }
}

// TODO: we would also like to be able to indicate how a component gets replicated (which channel; reliably or not, etc.)

#[derive(Default, Clone, Copy)]
pub enum ReplicationTarget {
    #[default]
    /// Broadcast updates to all clients
    All,
    /// Broadcast updates to all clients except the one specified
    AllExcept(ClientId),
    /// Send updates only to one specific client
    Only(ClientId),
}

impl ReplicationTarget {
    /// Return true if we should replicate to the specified client
    pub fn should_replicate_to(&self, client_id: ClientId) -> bool {
        match &self {
            ReplicationTarget::All => true,
            ReplicationTarget::AllExcept(id) => *id != client_id,
            ReplicationTarget::Only(id) => *id == client_id,
        }
    }
}

// NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
//  better to not add trait bounds on structs directly anyway

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ReplicationMessage<C, K> {
    // reliable
    // TODO: maybe include Vec<C> for SpawnEntity? All the components that already exist on this entity
    SpawnEntity(Entity, Vec<C>),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity, K),
    // unreliable
    EntityUpdate(Entity, Vec<C>),
}

pub trait ReplicationSend<P: Protocol>: Resource {
    fn entity_spawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_despawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    fn component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()>;

    fn component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_update_single_component(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_update(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    /// Any operation that needs to happen before we can send the replication messages
    /// (for example collecting the individual single component updates into a single message)
    fn prepare_replicate_send(&mut self);
}

// pub trait ReplicationReceive<P: Protocol>: Resource {
//     fn entity_spawn(
//         &mut self,
//         entity: Entity,
//         components: Vec<P::Components>,
//         replicate: &Replicate,
//     ) -> Result<()>;
//
// }
