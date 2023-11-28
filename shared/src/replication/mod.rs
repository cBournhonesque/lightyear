// for client: map from client_entity to server_entity

// bevy systems to add/update/remove components and entities
// (potentially let users choose which channel to use for these?)

use anyhow::Result;
use bevy::prelude::{Component, Entity, Resource};
use serde::{Deserialize, Serialize};

use crate::channel::builder::{Channel, EntityActionsChannel, EntityUpdatesChannel};
use crate::netcode::ClientId;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;

mod entity_map;
pub mod interpolation;
pub mod manager;
pub mod prediction;

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Component, Clone, Copy)]
pub struct Replicate {
    // TODO: be able to specify channel separately for entiy spawn/despawn and component insertion/removal?
    /// Optional: the channel to use for replicating entity updates.
    // pub channel: Option<ChannelKind>,

    /// The channel to use for replication (indicated by the generic type)
    // TODO: distinguish between replicating actions (inserts/etc.) vs updates
    // pub channel: ChannelKind,
    pub actions_channel: ChannelKind,
    pub updates_channel: ChannelKind,

    /// Which clients should this entity be replicated to
    pub replication_target: NetworkTarget,
    /// Which clients should predict this entity
    pub prediction_target: NetworkTarget,
    /// Which clients should interpolated this entity
    pub interpolation_target: NetworkTarget,
    // pub owner:
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates
    //  should we make this configurable?
}

#[derive(Default, Clone, Copy)]
/// NetworkTarget indicated which clients should receive some message or update
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except for one
    AllExcept(ClientId),
    /// Message sent to all clients
    All,
    /// Message sent to only one client
    Only(ClientId),
}

impl NetworkTarget {
    pub(crate) fn should_send_to(&self, client_id: &ClientId) -> bool {
        match self {
            NetworkTarget::All => true,
            NetworkTarget::AllExcept(id) => id != client_id,
            NetworkTarget::Only(id) => id == client_id,
            NetworkTarget::None => false,
        }
    }
}

impl Replicate {
    pub fn with_channel<C: Channel>() -> Self {
        Self {
            actions_channel: ChannelKind::of::<C>(),
            updates_channel: ChannelKind::of::<C>(),
            ..Default::default()
        }
    }
}

impl Default for Replicate {
    fn default() -> Self {
        Self {
            actions_channel: ChannelKind::of::<EntityActionsChannel>(),
            updates_channel: ChannelKind::of::<EntityUpdatesChannel>(),
            replication_target: NetworkTarget::All,
            prediction_target: NetworkTarget::None,
            interpolation_target: NetworkTarget::None,
        }
    }
}

impl NetworkTarget {
    /// Return true if we should replicate to the specified client
    pub fn should_replicate_to(&self, client_id: ClientId) -> bool {
        match &self {
            NetworkTarget::All => true,
            NetworkTarget::AllExcept(id) => *id != client_id,
            NetworkTarget::Only(id) => *id == client_id,
            NetworkTarget::None => false,
        }
    }
}

// NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
//  better to not add trait bounds on structs directly anyway

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub(crate) enum ReplicationMessage<C, K> {
    // reliable
    // TODO: maybe include Vec<C> for SpawnEntity? All the components that already exist on this entity
    SpawnEntity(Entity, Vec<C>),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity, K),
    // sequenced unreliable
    // TODO: add the tick of the update? maybe this makes no sense if we gather updates only at the end of the tick
    EntityUpdate(Entity, Vec<C>),
}

pub trait ReplicationSend<P: Protocol>: Resource {
    fn entity_spawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_despawn(&mut self, entity: Entity, replicate: &Replicate) -> Result<()>;

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
