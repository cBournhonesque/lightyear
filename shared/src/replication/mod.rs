// for client: map from client_entity to server_entity

// bevy systems to add/update/remove components and entities
// (potentially let users choose which channel to use for these?)

use crate::netcode::ClientId;
use crate::ChannelKind;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;

#[derive(Component, Clone, Copy, Default)]
pub struct Replicate {
    /// Optional: the channel to use for replication.
    pub channel: Option<ChannelKind>,
    pub target: ReplicationTarget,
}

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
