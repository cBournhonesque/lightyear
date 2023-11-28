//! Replication module
use anyhow::Result;
use bevy::prelude::{Component, Entity, Resource};
use serde::{Deserialize, Serialize};

use crate::channel::builder::{Channel, EntityActionsChannel, EntityUpdatesChannel};
use crate::netcode::ClientId;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::replication::components::Replicate;

/// Components used for replication
pub mod components;

/// Map between local and remote entities
mod entity_map;

/// General struct handling replication
pub mod manager;

/// Bevy [`bevy::prelude::Resource`]s used for replication
pub mod resources;

/// Bevy [`bevy::prelude::System`]s used for replication
pub mod systems;

// NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
//  better to not add trait bounds on structs directly anyway
#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ReplicationMessage<C, K> {
    // TODO: maybe include Vec<C> for SpawnEntity? All the components that already exist on this entity
    SpawnEntity(Entity, Vec<C>),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity, K),
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
