use std::{
    collections::{hash_set::Iter, HashSet},
    hash::Hash,
    net::SocketAddr,
};

use crate::shared::BigMapKey;

use crate::server::{RoomKey, Server};

// UserKey
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
pub struct UserKey(u64);

impl BigMapKey for UserKey {
    fn to_u64(&self) -> u64 {
        self.0
    }

    fn from_u64(value: u64) -> Self {
        UserKey(value)
    }
}

// User

#[derive(Clone)]
pub struct User {
    pub address: SocketAddr,
    rooms_cache: HashSet<RoomKey>,
}

impl User {
    pub fn new(address: SocketAddr) -> User {
        User {
            address,
            rooms_cache: HashSet::new(),
        }
    }

    pub(crate) fn cache_room(&mut self, room_key: &RoomKey) {
        self.rooms_cache.insert(*room_key);
    }

    pub(crate) fn uncache_room(&mut self, room_key: &RoomKey) {
        self.rooms_cache.remove(room_key);
    }

    pub(crate) fn room_keys(&self) -> Iter<RoomKey> {
        self.rooms_cache.iter()
    }

    pub(crate) fn room_count(&self) -> usize {
        self.rooms_cache.len()
    }
}

// UserRef

pub struct UserRef<'s> {
    server: &'s Server,
    key: UserKey,
}

impl<'s> UserRef<'s> {
    pub fn new(server: &'s Server, key: &UserKey) -> Self {
        UserRef { server, key: *key }
    }

    pub fn key(&self) -> UserKey {
        self.key
    }

    pub fn address(&self) -> SocketAddr {
        self.server.user_address(&self.key).unwrap()
    }

    pub fn room_count(&self) -> usize {
        self.server.user_rooms_count(&self.key).unwrap()
    }

    /// Returns an iterator of all the keys of the [`Room`]s the User belongs to
    pub fn room_keys(&self) -> impl Iterator<Item = &RoomKey> {
        self.server.user_room_keys(&self.key).unwrap()
    }
}

// UserMut
pub struct UserMut<'s> {
    server: &'s mut Server,
    key: UserKey,
}

impl<'s> UserMut<'s> {
    pub fn new(server: &'s mut Server, key: &UserKey) -> Self {
        UserMut { server, key: *key }
    }

    pub fn key(&self) -> UserKey {
        self.key
    }

    pub fn address(&self) -> SocketAddr {
        self.server.user_address(&self.key).unwrap()
    }

    pub fn disconnect(&mut self) {
        self.server.user_disconnect(&self.key);
    }

    // Rooms

    pub fn enter_room(&mut self, room_key: &RoomKey) -> &mut Self {
        self.server.room_add_user(room_key, &self.key);

        self
    }

    pub fn leave_room(&mut self, room_key: &RoomKey) -> &mut Self {
        self.server.room_remove_user(room_key, &self.key);

        self
    }

    pub fn room_count(&self) -> usize {
        self.server.user_rooms_count(&self.key).unwrap()
    }

    /// Returns an iterator of all the keys of the [`Room`]s the User belongs to
    pub fn room_keys(&self) -> impl Iterator<Item = &RoomKey> {
        self.server.user_room_keys(&self.key).unwrap()
    }
}
