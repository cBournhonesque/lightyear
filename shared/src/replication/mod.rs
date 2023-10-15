// for client: map from client_entity to server_entity

// bevy systems to add/update/remove components and entities
// (potentially let users choose which channel to use for these?)

use std::marker::PhantomData;

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bitcode::__private::Serialize;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use lightyear_derive::ChannelInternal;

use crate::netcode::ClientId;
use crate::{BitSerializable, ComponentProtocol, ComponentProtocolKind, ReadBuffer, WriteBuffer};

mod entity_map;
pub mod manager;

/// Default channel to replicate entity updates reliably
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
#[derive(ChannelInternal)]
pub struct DefaultReliableChannel;

/// Component that indicates that an entity should be replicated.
#[derive(Component, Clone, Copy, Default)]
pub struct Replicate<C = DefaultReliableChannel> {
    // TODO: be able to specify channel separately for entiy spawn/despawn and component insertion/removal?
    /// Optional: the channel to use for replicating entity updates.
    // pub channel: Option<ChannelKind>,

    /// The channel to use for replication (indicated by the generic type)
    pub channel: PhantomData<C>,
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

// NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
//  better to not add trait bounds on structs directly anyway
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
