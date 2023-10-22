use std::collections::hash_map::Entry;
use std::collections::HashMap;

use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::World;
use bevy_ecs::world::EntityMut;

use crate::replication::Replicate;

#[derive(Default)]
pub struct EntityMap {
    remote_to_local: HashMap<Entity, Entity>,
    local_to_remote: HashMap<Entity, Entity>,
}

impl EntityMap {
    #[inline]
    pub fn insert(&mut self, remote_entity: Entity, local_entity: Entity) {
        self.remote_to_local.insert(remote_entity, local_entity);
        self.local_to_remote.insert(local_entity, remote_entity);
    }

    pub fn get_local(&mut self, remote_entity: Entity) -> Option<&Entity> {
        self.remote_to_local.get(&remote_entity)
    }

    pub fn get_remote(&mut self, local_entity: Entity) -> Option<&Entity> {
        self.local_to_remote.get(&local_entity)
    }

    /// Get the corresponding local entity for a given remote entity, or create it if it doesn't exist.
    pub(super) fn get_by_remote_or_spawn<'a>(
        &mut self,
        world: &'a mut World,
        remote_entity: Entity,
    ) -> EntityMut<'a> {
        match self.remote_to_local.entry(remote_entity) {
            Entry::Occupied(entry) => world.entity_mut(*entry.get()),
            Entry::Vacant(entry) => {
                // TODO: why could this happen?
                // - new connection; all existing entity are transmitted
                // - entity got despawned on client while this update was being transmitted, in which case we don't want to respawn.
                let local_entity = world.spawn(Replicate::default());
                entry.insert(local_entity.id());
                self.local_to_remote
                    .insert(local_entity.id(), remote_entity);
                local_entity
            }
        }
    }

    pub(super) fn remove_by_remote(&mut self, remote_entity: Entity) -> Option<Entity> {
        let local_entity = self.remote_to_local.remove(&remote_entity);
        if let Some(local_entity) = local_entity {
            self.local_to_remote.remove(&local_entity);
        }
        local_entity
    }

    #[inline]
    pub fn to_local(&self) -> &HashMap<Entity, Entity> {
        &self.remote_to_local
    }

    #[inline]
    pub fn to_remote(&self) -> &HashMap<Entity, Entity> {
        &self.local_to_remote
    }

    fn clear(&mut self) {
        self.local_to_remote.clear();
        self.remote_to_local.clear();
    }
}
