use std::collections::HashMap;

use bevy_ecs::entity::Entity;
use bevy_ecs::system::Resource;

/// Maps server entities to client entities and vice versa.
///
/// Used only on client.
#[derive(Default, Resource)]
pub struct NetworkEntityMap {
    server_to_client: HashMap<Entity, Entity>,
    client_to_server: HashMap<Entity, Entity>,
}

impl NetworkEntityMap {
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
