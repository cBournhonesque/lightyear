use crate::client::{Connecting, PeerMetadata};
use bevy::ecs::component::HookContext;
use bevy::ecs::error::HandleError;
use bevy::ecs::error::{ignore, panic, CommandWithEntity};
use bevy::ecs::relationship::{Relationship, RelationshipHookMode, RelationshipSourceCollection};
use bevy::ecs::system::entity_command;
use bevy::ecs::world::DeferredWorld;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use tracing::warn;

use crate::prelude::NetworkTarget;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec, vec::Vec};
use lightyear_core::id::PeerId;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use smallvec::SmallVec;

/// Marker component to identify this entity as a Client
#[derive(Component, Default, Debug, PartialEq, Eq, Reflect)]
#[reflect(Component, PartialEq, Debug)]
#[component(on_despawn = Server::on_despawn)]
#[component(on_replace = Server::on_replace)]
pub struct Server {
    // TODO: replace this with EntityIndexSet in 0.17
    /// The clients that are connecting/connected to this server.
    /// (They all have a ClientOf component)
    ///
    /// Accessing this directly is unsafe, and is only necessary to solve some issues with split borrows
    pub clients: Vec<Entity>,
}

impl Server {
    /// Calls func on each value that matches the provided `target`
    pub fn apply_targets(
        &self,
        target: &NetworkTarget,
        mapping: &HashMap<PeerId, Entity>,
        func: &mut impl FnMut(Entity)
    ) {
        match target {
            NetworkTarget::All => self.clients.iter().for_each(|e| func(e)),
            NetworkTarget::AllExceptSingle(client_id) => {
                let except_entity = mapping.get(client_id).unwrap_or(&Entity::PLACEHOLDER);
                self.clients.iter()
                    .filter(|e| e != except_entity)
                    .for_each(|e| func(e))
            }
            NetworkTarget::AllExcept(client_ids) => {
                let entity_ids = client_ids.iter()
                    .map(|p| *mapping.get(p).unwrap_or(&Entity::PLACEHOLDER))
                    .collect::<SmallVec<[Entity; 4]>>();
                self.clients.iter()
                    .filter(|e| !entity_ids.contains(e))
                    .for_each(|e| func(e))
            }
            NetworkTarget::Single(client_id) => {
                let entity = mapping.get(client_id).unwrap_or(&Entity::PLACEHOLDER);
                if let Some(e) = self.clients.iter().find(|e| e == entity) {
                    func(e)
                }
            },
            NetworkTarget::Only(client_ids) => {
                let entity_ids = client_ids.iter()
                    .map(|p| *mapping.get(p).unwrap_or(&Entity::PLACEHOLDER))
                    .collect::<SmallVec<[Entity; 4]>>();
                self.clients.iter()
                    .filter(|e| entity_ids.contains(e))
                    .for_each(|e| func(e))
            }
            NetworkTarget::None => {}
        }
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component, PartialEq, Debug, Clone)]
// every ClientOf starts as Connecting until the server confirms the connection
#[require(Connecting)]
#[component(on_insert = ClientOf::on_insert)]
#[component(on_replace = ClientOf::on_replace)]
pub struct ClientOf {
    /// The server entity that this client is connected to
    pub server: Entity,
}

/// We implement Relationship manually because we also want to update information related to the ClientId in the `Server` component
impl Relationship for ClientOf {
    type RelationshipTarget = Server;

    fn get(&self) -> Entity {
        self.server
    }

    fn from(entity: Entity) -> Self {
        ClientOf {
            server: entity
        }
    }
}

impl ClientOf {

    /// The `on_insert` component hook that maintains the [`Relationship`] / [`RelationshipTarget`] connection.
    fn on_insert(
        mut world: DeferredWorld,
        HookContext {
            entity,
            caller,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => {
                return;
            }
        }
        let client_of = world.entity(entity).get::<Self>().unwrap();
        let target_entity = client_of.server;
        if target_entity == entity {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} points to itself. The invalid {} relationship has been removed.",
                caller.map(|location| format!("{location}: ")).unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
            return;
        }
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) =
                target_entity_mut.get_mut::<Server>()
            {
                relationship_target.collection_mut_risky().add(entity);
            } else {
                let mut target = <Server as RelationshipTarget>::with_capacity(1);
                target.collection_mut_risky().add(entity);
                world.commands().entity(target_entity).insert(target);
            }
        } else {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} relates to an entity that does not exist. The invalid {} relationship has been removed.",
                caller.map(|location| format!("{location}: ")).unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
        }

        // make client_of's timeline match the server's timeline
        let server_timeline = world.get::<LocalTimeline>(target_entity).unwrap().clone();
        let mut timeline = world.get_mut::<LocalTimeline>(entity).unwrap();
        *timeline = server_timeline;
    }

    fn on_replace(
        mut world: DeferredWorld,
        HookContext {
            entity,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => {
                return;
            }
        }
        let target_entity = world.entity(entity).get::<Self>().unwrap().get();
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) =
                target_entity_mut.get_mut::<Server>()
            {
                RelationshipSourceCollection::remove(relationship_target.collection_mut_risky(), entity);
            }
        }
    }
}


impl RelationshipTarget for Server {
    const LINKED_SPAWN: bool = true;
    type Relationship = ClientOf;
    type Collection = Vec<Entity>;

    fn collection(&self) -> &Self::Collection {
        &self.clients
    }

    fn collection_mut_risky(&mut self) -> &mut Self::Collection {
        &mut self.clients
    }

    fn from_collection_risky(collection: Self::Collection) -> Self {
        Self {
            clients: collection,
        }
    }
}

impl Server {
    /// The `on_replace` component hook that maintains the [`Relationship`] / [`RelationshipTarget`] connection.
    // note: think of this as "on_drop"
    fn on_replace(mut world: DeferredWorld, HookContext { entity, caller, .. }: HookContext) {
        let (entities, mut commands) = world.entities_and_commands();
        let relationship_target = entities.get(entity).unwrap().get::<Self>().unwrap();
        for source_entity in relationship_target.iter() {
            if entities.get(source_entity).is_ok() {
                commands.queue(
                    entity_command::remove::<ClientOf>()
                        .with_entity(source_entity)
                        .handle_error_with(ignore),
                );
            } else {
                warn!(
                    "{}Tried to despawn non-existent entity {}",
                    caller
                        .map(|location| format!("{location}: "))
                        .unwrap_or_default(),
                    source_entity
                );
            }
        }
    }

    /// The `on_despawn` component hook that despawns entities stored in an entity's [`RelationshipTarget`] when
    /// that entity is despawned.
    // note: think of this as "on_drop"
    fn on_despawn(mut world: DeferredWorld, HookContext { entity, caller, .. }: HookContext) {
        let (entities, mut commands) = world.entities_and_commands();
        let relationship_target = entities.get(entity).unwrap().get::<Self>().unwrap();
        for source_entity in relationship_target.iter() {
            if entities.get(source_entity).is_ok() {
                commands.queue(
                    entity_command::despawn()
                        .with_entity(source_entity)
                        .handle_error_with(ignore),
                );
            } else {
                warn!(
                    "{}Tried to despawn non-existent entity {}",
                    caller
                        .map(|location| format!("{location}: "))
                        .unwrap_or_default(),
                    source_entity
                );
            }
        }
    }
}