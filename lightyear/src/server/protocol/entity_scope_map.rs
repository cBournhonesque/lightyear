use std::{
    collections::{HashMap, HashSet},
};
use bevy_ecs::entity::Entity;
use crate::server::UserKey;


pub struct EntityScopeMap {
    entities_of_user: HashMap<UserKey, HashSet<Entity>>,
    users_of_entity: HashMap<Entity, HashSet<UserKey>>,
    main_map: HashMap<(UserKey, Entity), bool>,
}

impl EntityScopeMap {
    pub fn new() -> Self {
        Self {
            main_map: HashMap::new(),
            entities_of_user: HashMap::new(),
            users_of_entity: HashMap::new(),
        }
    }

    pub fn get(&self, user_key: &UserKey, entity: &Entity) -> Option<&bool> {
        let key = (*user_key, *entity);

        self.main_map.get(&key)
    }

    pub fn insert(&mut self, user_key: UserKey, entity: Entity, in_scope: bool) {
        self.entities_of_user
            .entry(user_key)
            .or_insert_with(|| HashSet::new());
        self.users_of_entity
            .entry(entity)
            .or_insert_with(HashSet::new);

        self.entities_of_user
            .get_mut(&user_key)
            .unwrap()
            .insert(entity);
        self.users_of_entity
            .get_mut(&entity)
            .unwrap()
            .insert(user_key);

        self.main_map.insert((user_key, entity), in_scope);
    }

    pub fn remove_user(&mut self, user_key: &UserKey) {
        if let Some(entities) = self.entities_of_user.get(user_key) {
            for entity in entities {
                if let Some(users) = self.users_of_entity.get_mut(entity) {
                    users.remove(user_key);
                    self.main_map.remove(&(*user_key, *entity));
                }
            }
        }

        self.entities_of_user.remove(user_key);
    }

    pub fn remove_entity(&mut self, entity: &Entity) {
        if let Some(users) = self.users_of_entity.get(entity) {
            for user in users {
                if let Some(entities) = self.entities_of_user.get_mut(user) {
                    entities.remove(entity);
                    self.main_map.remove(&(*user, *entity));
                }
            }
        }

        self.users_of_entity.remove(entity);
    }
}
