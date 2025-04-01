use crate::id::ClientId;
use bevy::ecs::component::HookContext;
use bevy::ecs::error::HandleError;
use bevy::ecs::error::{ignore, panic, CommandWithEntity};
use bevy::ecs::relationship::{Relationship, RelationshipHookMode};
use bevy::ecs::system::entity_command;
use bevy::ecs::world::DeferredWorld;
use bevy::platform_support::collections::HashMap;
use bevy::prelude::format;
use bevy::prelude::{Component, Entity, EntityWorldMut, RelationshipTarget};
use lightyear_messages::MessageManager;
use tracing::warn;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

/// Marker component to identify this entity as a Client
#[derive(Component, Default, Debug, PartialEq, Eq)]
#[component(on_despawn = Server::on_despawn)]
#[component(on_replace = Server::on_replace)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "bevy_reflect", reflect(Component, FromWorld, Default))]
pub struct Server {
    /// The server entity that this client is connected to
    clients: Vec<Entity>,
    client_map: HashMap<ClientId, Entity>,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
#[require(MessageManager)]
#[component(on_insert = ClientOf::on_insert)]
#[component(on_replace = ClientOf::on_replace)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(
    feature = "bevy_reflect",
    reflect(Component, PartialEq, Debug, FromWorld, Clone)
)]
pub struct ClientOf {
    /// The server entity that this client is connected to
    pub server: Entity,
    /// The client id of the client
    pub id: ClientId,
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
    fn id(&self) -> ClientId {
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
                if relationship_target.len() == 0 {
                    if let Ok(mut entity) = world.commands().get_entity(target_entity) {
                        // this "remove" operation must check emptiness because in the event that an identical
                        // relationship is inserted on top, this despawn would result in the removal of that identical
                        // relationship ... not what we want!
                        entity.queue(|mut entity: EntityWorldMut| {
                            if entity
                                .get::<Server>()
                                .is_some_and(RelationshipTarget::is_empty)
                            {
                                entity.remove::<Server>();
                            }
                        });
                    }
                }
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

    // SAFETY: this should only be called as part of the relationship hooks
    unsafe fn add_client(&mut self, client: Entity, id: ClientId) {
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