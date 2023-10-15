use bevy_ecs::prelude::{Entity, World};
use lightyear_shared::replication::ReplicationMessage;
use lightyear_shared::Protocol;
use std::collections::HashMap;

pub struct EntityMap {
    server_to_client: HashMap<Entity, Entity>,
    client_to_server: HashMap<Entity, Entity>,
}

impl EntityMap {
    #[inline]
    pub fn insert(&mut self, server_entity: Entity, client_entity: Entity) {
        self.server_to_client.insert(server_entity, client_entity);
        self.client_to_server.insert(client_entity, server_entity);
    }

    // /// Get the corresponding client entity for a given server entity, or create it if it doesn't exist.
    // pub(super) fn get_by_server_or_spawn<'a>(
    //     &mut self,
    //     world: &'a mut World,
    //     server_entity: Entity,
    // ) -> EntityMut<'a> {
    //     match self.server_to_client.entry(server_entity) {
    //         Entry::Occupied(entry) => world.entity_mut(*entry.get()),
    //         Entry::Vacant(entry) => {
    //             let client_entity = world.spawn(Replicate);
    //             entry.insert(client_entity.id());
    //             self.client_to_server
    //                 .insert(client_entity.id(), server_entity);
    //             client_entity
    //         }
    //     }
    // }

    pub(super) fn remove_by_server(&mut self, server_entity: Entity) -> Option<Entity> {
        let client_entity = self.server_to_client.remove(&server_entity);
        if let Some(client_entity) = client_entity {
            self.client_to_server.remove(&client_entity);
        }
        client_entity
    }

    #[inline]
    pub fn to_client(&self) -> &HashMap<Entity, Entity> {
        &self.server_to_client
    }

    #[inline]
    pub fn to_server(&self) -> &HashMap<Entity, Entity> {
        &self.client_to_server
    }

    fn clear(&mut self) {
        self.client_to_server.clear();
        self.server_to_client.clear();
    }
}

pub struct ReplicationManager<P: Protocol> {
    entity_map: EntityMap,
}

// flow:
// - recv packets to store all received stuff inside buffers
// - receive to read from buffers into Events
// - read from events to actually apply the replication to the world, and maybe do additional book-keeping
//   also generate bevy events
// TODO: do we need to send back messages to inform about how the replication went?
impl<P: Protocol> ReplicationManager<P> {
    fn replicate(
        &mut self,
        world: &mut World,
        message: ReplicationMessage<P::Components, P::ComponentKinds>,
    ) {
        match message {
            ReplicationMessage::SpawnEntity(server_entity, components) => {
                // TODO: convert components into bundle/tuple
                // TODO: add Replicate component (or can just make it automatically replicated? or is it a waste)
                let client_entity = world.spawn(components.into()).id();
                self.entity_map.insert(server_entity, client_entity);
            }
            ReplicationMessage::DespawnEntity(server_entity) => {
                if let Some(client_entity) = self.entity_map.remove_by_server(server_entity) {
                    world.despawn(client_entity);
                }
            }
            ReplicationMessage::InsertComponent(_, _) => {}
            ReplicationMessage::RemoveComponent(_, _) => {}
            ReplicationMessage::EntityUpdate(_, _) => {}
        }
    }
}
