use crate::client::Connecting;
use bevy::ecs::component::HookContext;
use bevy::ecs::error::HandleError;
use bevy::ecs::error::{ignore, panic, CommandWithEntity};
use bevy::ecs::relationship::{Relationship, RelationshipHookMode};
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

/// Marker component to identify this entity as a Client
#[derive(Component, Default, Debug, PartialEq, Eq, Reflect)]
#[reflect(Component, PartialEq, Debug)]
#[component(on_despawn = Server::on_despawn)]
#[component(on_replace = Server::on_replace)]
pub struct Server {
    /// The server entity that this client is connected to
    ///
    /// Accessing this directly is unsafe, and is only necessary to solve some issues with split borrows
    pub clients: Vec<Entity>,
    pub client_map: HashMap<PeerId, Entity>,
}

impl Server {
    pub fn targets<'a: 'b, 'b>(&'a self, target: &'b NetworkTarget) -> Box<dyn Iterator<Item = Entity> + 'b> {
        match target {
            NetworkTarget::All => Box::new(self.client_map.values().copied()),
            NetworkTarget::AllExceptSingle(client_id) =>
                Box::new(self.client_map
                    .iter()
                    .filter(move |(peer_id, _)| *peer_id != client_id)
                    .map(|(_, e)| *e)),
            NetworkTarget::AllExcept(client_ids) => Box::new(
                self.client_map
                    .iter()
                    .filter(move |(peer_id, _)| !client_ids.contains(peer_id))
                    .map(|(_, e)| *e)),
            NetworkTarget::Single(client_id) => {
                Box::new(self.client_map.get(client_id).copied().into_iter())
            },
            NetworkTarget::Only(client_ids) => Box::new(
                self.client_map
                    .iter()
                    .filter(move |(peer_id, _)| client_ids.contains(peer_id))
                    .map(|(_, e)| *e)),
            NetworkTarget::None => Box::new(core::iter::empty()),
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
    /// The client id of the client
    pub id: PeerId,
}

/// We implement Relationship manually because we also want to update information related to the ClientId in the `Server` component
impl Relationship for ClientOf {
    type RelationshipTarget = Server;

    fn get(&self) -> Entity {
        self.server
    }

    fn from(entity: Entity) -> Self {
        panic!("The `from` function should never be called.");
    }
}

impl ClientOf {
    fn id(&self) -> PeerId {
        self.id
    }

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
        let client = client_of.id;
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
                // SAFETY: we are calling this as part of the relationship hooks
                unsafe { relationship_target.add_client(entity, client) };
            } else {
                let mut target = <Server as RelationshipTarget>::with_capacity(1);
                // SAFETY: we are calling this as part of the relationship hooks
                unsafe { target.add_client(entity, client) };
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
                unsafe { relationship_target.remove_client(entity) };
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
        panic!("This function should never be called. Use `from_collection` instead.");
    }
}

impl Server {

    pub fn get_client(&self, id: PeerId) -> Option<Entity> {
        self.client_map.get(&id).copied()
    }

    // SAFETY: this should only be called as part of the relationship hooks
    unsafe fn add_client(&mut self, client: Entity, id: PeerId) {
        self.clients.push(client);
        self.client_map.insert(id, client);
    }

    // SAFETY: this should only be called as part of the relationship hooks
    unsafe fn remove_client(&mut self, client: Entity) {
        self.clients.retain(|e| *e != client);
        self.client_map.extract_if(|_, v| *v == client).next();
    }

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