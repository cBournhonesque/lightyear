// for client: map from client_entity to server_entity

// bevy systems to add/update/remove components and entities
// (potentially let users choose which channel to use for these?)

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;

use crate::netcode::ClientId;
use crate::ChannelKind;

/// Component that indicates that an entity should be replicated.
#[derive(Component, Clone, Copy, Default)]
pub struct Replicate {
    // TODO: be able to specify channel separately for entiy spawn/despawn and component insertion/removal?
    /// Optional: the channel to use for replicating entity updates.
    pub channel: Option<ChannelKind>,
    pub target: ReplicationTarget,
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

pub enum ReplicationMessage<C> {
    SpawnEntity(Entity),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity), // TODO: add type of component
    EntityUpdate(Entity, Vec<C>),
}
