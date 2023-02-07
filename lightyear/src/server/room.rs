use std::{
    collections::{hash_set::Iter, HashSet, VecDeque},
    hash::Hash,
};
use bevy_ecs::entity::Entity;

use crate::shared::{BigMapKey, Channel, Channels, Message};

use super::user::UserKey;

// RoomKey
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct RoomKey(u64);

impl BigMapKey for RoomKey {
    fn to_u64(&self) -> u64 {
        self.0
    }

    fn from_u64(value: u64) -> Self {
        RoomKey(value)
    }
}

// Room
pub struct Room {
    users: HashSet<UserKey>,
    entities: HashSet<Entity>,
    entity_removal_queue: VecDeque<(UserKey, Entity)>,
}

impl Room {
    pub(crate) fn new() -> Room {
        Room {
            users: HashSet::new(),
            entities: HashSet::new(),
            entity_removal_queue: VecDeque::new(),
        }
    }

    // Users

    pub(crate) fn has_user(&self, user_key: &UserKey) -> bool {
        self.users.contains(user_key)
    }

    pub(crate) fn subscribe_user(&mut self, user_key: &UserKey) {
        self.users.insert(*user_key);
    }

    pub(crate) fn unsubscribe_user(&mut self, user_key: &UserKey) {
        self.users.remove(user_key);
        for entity in self.entities.iter() {
            self.entity_removal_queue.push_back((*user_key, *entity));
        }
    }

    pub(crate) fn user_keys(&self) -> Iter<UserKey> {
        self.users.iter()
    }

    pub(crate) fn users_count(&self) -> usize {
        self.users.len()
    }

    // Entities

    pub(crate) fn add_entity(&mut self, entity: &Entity) {
        self.entities.insert(*entity);
    }

    pub(crate) fn remove_entity(&mut self, entity: &Entity) -> bool {
        if self.entities.remove(entity) {
            for user_key in self.users.iter() {
                self.entity_removal_queue.push_back((*user_key, *entity));
            }
            true
        } else {
            panic!("Room does not contain Entity");
        }
    }

    pub(crate) fn entities(&self) -> Iter<Entity> {
        self.entities.iter()
    }

    pub(crate) fn pop_entity_removal_queue(&mut self) -> Option<(UserKey, Entity)> {
        self.entity_removal_queue.pop_front()
    }

    pub(crate) fn entities_count(&self) -> usize {
        self.entities.len()
    }
}

// room references

use super::server::Server;

// RoomRef

pub struct RoomRef<'s> {
    server: &'s Server,
    key: RoomKey,
}

impl<'s> RoomRef<'s> {
    pub fn new(server: &'s Server, key: &RoomKey) -> Self {
        RoomRef { server, key: *key }
    }

    pub fn key(&self) -> RoomKey {
        self.key
    }

    // Users

    pub fn has_user(&self, user_key: &UserKey) -> bool {
        self.server.room_has_user(&self.key, user_key)
    }

    pub fn users_count(&self) -> usize {
        self.server.room_users_count(&self.key)
    }

    /// Returns an iterator of the [`UserKey`] for Users that belong in the [`Room`]
    pub fn user_keys(&self) -> impl Iterator<Item = &UserKey> {
        self.server.room_user_keys(&self.key)
    }

    // Entities

    pub fn has_entity(&self, entity: &Entity) -> bool {
        self.server.room_has_entity(&self.key, entity)
    }

    pub fn entities_count(&self) -> usize {
        self.server.room_entities_count(&self.key)
    }
}

// RoomMut
pub struct RoomMut<'s> {
    server: &'s mut Server,
    key: RoomKey,
}

impl<'s> RoomMut<'s> {
    pub fn new(server: &'s mut Server, key: &RoomKey) -> Self {
        RoomMut { server, key: *key }
    }

    pub fn key(&self) -> RoomKey {
        self.key
    }

    pub fn destroy(&mut self) {
        self.server.room_destroy(&self.key);
    }

    // Users

    pub fn has_user(&self, user_key: &UserKey) -> bool {
        self.server.room_has_user(&self.key, user_key)
    }

    pub fn add_user(&mut self, user_key: &UserKey) -> &mut Self {
        self.server.room_add_user(&self.key, user_key);

        self
    }

    pub fn remove_user(&mut self, user_key: &UserKey) -> &mut Self {
        self.server.room_remove_user(&self.key, user_key);

        self
    }

    pub fn users_count(&self) -> usize {
        self.server.room_users_count(&self.key)
    }

    /// Returns an iterator of the [`UserKey`] for Users that belong in the [`Room`]
    pub fn user_keys(&self) -> impl Iterator<Item = &UserKey> {
        self.server.room_user_keys(&self.key)
    }

    // Entities

    pub fn has_entity(&self, entity: &Entity) -> bool {
        self.server.room_has_entity(&self.key, entity)
    }

    pub fn add_entity(&mut self, entity: &Entity) -> &mut Self {
        self.server.room_add_entity(&self.key, entity);

        self
    }

    pub fn remove_entity(&mut self, entity: &Entity) -> &mut Self {
        self.server.room_remove_entity(&self.key, entity);

        self
    }

    pub fn entities_count(&self) -> usize {
        self.server.room_entities_count(&self.key)
    }

    // Messages

    pub fn broadcast_message<C: Channel, M: Message>(&mut self, message: M) {
        self.server.room_broadcast_message(
            &Channels::type_to_id::<C>(),
            Box::new(message),
            &self.key,
        );
    }
}
