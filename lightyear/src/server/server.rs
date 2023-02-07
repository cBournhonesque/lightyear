use std::{
    collections::HashMap, net::SocketAddr, panic,
};
use std::any::Any;
use std::collections::hash_set::Iter;

#[cfg(feature = "bevy_support")]
use bevy_ecs::prelude::Resource;

use bevy_ecs::world::World;
use bevy_ecs::entity::Entity;
use bevy_ecs::component::{Component, ComponentId, Components};

use naia_server_socket::{ServerAddrs, Socket};
use tracing::warn;
use lightyear_serde::BitWriter;
use crate::shared::{BigMap, Channel, ChannelId, Channels, Instant, Message, PacketType,
                    Protocol, Replicate, ReplicateSafe, StandardHeader, Tick, Timer};

use crate::server::{
    connection::{
        connection::Connection,
        handshake_manager::HandshakeManager,
        io::Io,
    },
    protocol::entity_scope_map::EntityScopeMap,
    tick::tick_manager::TickManager,
};
use crate::server::internal::HandshakeResult;

use super::{
    error::NaiaServerError,
    events::Events,
    room::{Room, RoomKey, RoomMut, RoomRef},
    server_config::ServerConfig,
    user::{User, UserKey, UserMut, UserRef},
    user_scope::UserScopeMut,
};

/// A server that uses either UDP or WebRTC communication to send/receive
/// messages to/from connected clients, and syncs registered entities to
/// clients to whom they are in-scope
#[cfg_attr(feature = "bevy_support", derive(ResourcEntity))]
pub struct Server {
    // Config
    server_config: ServerConfig,
    socket: Socket,
    io: Io,
    heartbeat_timer: Timer,
    timeout_timer: Timer,
    ping_timer: Timer,
    handshake_manager: HandshakeManager,
    // Users
    users: BigMap<UserKey, User>,
    user_connections: HashMap<SocketAddr, Connection>,
    // Rooms
    rooms: BigMap<RoomKey, Room>,
    // Entities
    entity_scope_map: EntityScopeMap,
    // Events
    incoming_events: Events,
    // Ticks
    tick_manager: Option<TickManager>,
}

impl Server {
    /// Create a new Server
    pub fn new(server_config: ServerConfig, protocol: Protocol) -> Self {
        let socket = Socket::new(&protocol.socket);

        let tick_manager = { protocol.tick_interval.map(TickManager::new) };

        Server {
            // Config
            server_config: server_config.clone(),
            // Connection
            socket,
            io: Io::new(
                &server_config.connection.bandwidth_measure_duration,
                &protocol.compression,
            ),
            heartbeat_timer: Timer::new(server_config.connection.heartbeat_interval),
            timeout_timer: Timer::new(server_config.connection.disconnection_timeout_duration),
            ping_timer: Timer::new(server_config.connection.ping.ping_interval),
            handshake_manager: HandshakeManager::new(server_config.require_auth),
            // Users
            users: BigMap::default(),
            user_connections: HashMap::new(),
            // Rooms
            rooms: BigMap::default(),
            entity_scope_map: EntityScopeMap::new(),
            // Events
            incoming_events: Events::new(),
            // Ticks
            tick_manager,
        }
    }

    /// Listen at the given addresses
    pub fn listen(&mut self, server_addrs: &ServerAddrs) {
        self.socket.listen(server_addrs);
        self.io
            .load(self.socket.packet_sender(), self.socket.packet_receiver());
    }

    /// Returns whether or not the Server has initialized correctly and is
    /// listening for Clients
    pub fn is_listening(&self) -> bool {
        self.io.is_loaded()
    }

    /// Must be called regularly, maintains connection to and receives messages
    /// from all Clients
    pub fn receive(&mut self) -> Events {
        // Need to run this to maintain connection with all clients, and receive packets
        // until none left
        self.maintain_socket();

        // tick event
        let mut did_tick = false;
        if let Some(tick_manager) = &mut self.tick_manager {
            if tick_manager.recv_server_tick() {
                did_tick = true;
            }
        }

        // loop through all connections, receive Messages
        let mut user_addresses: Vec<SocketAddr> = self.user_connections.keys().copied().collect();
        fastrand::shuffle(&mut user_addresses);

        for user_address in &user_addresses {
            let connection = self.user_connections.get_mut(user_address).unwrap();

            // receive messages from anyone
            connection
                .base
                .message_manager
                .receive_messages(&mut self.incoming_events);
        }

        // receive (retrieve from buffer) tick buffered messages for the current server tick
        if did_tick {
            // Receive Tick Buffered Messages
            for user_address in &user_addresses {
                let connection = self.user_connections.get_mut(user_address).unwrap();

                connection.tick_buffer.receive_messages(
                    &self.tick_manager.as_ref().unwrap().server_tick(),
                    &mut self.incoming_events,
                );
            }

            self.incoming_events.push_tick();
        }

        // return all received messages and reset the buffer
        std::mem::take(&mut self.incoming_events)
    }

    // Connections

    /// Accepts an incoming Client User, allowing them to establish a connection
    /// with the Server
    pub fn accept_connection(&mut self, user_key: &UserKey) {
        if let Some(user) = self.users.get(user_key) {
            let new_connection = Connection::new(
                &self.server_config.connection,
                user.address,
                user_key,
            );
            // send connectaccept response
            let mut writer = self.handshake_manager.write_connect_response();
            match self.io.send_writer(&user.address, &mut writer) {
                Ok(()) => {}
                Err(_) => {
                    // TODO: pass this on and handle above
                    warn!(
                        "Server Error: Cannot send connect response packet to {}",
                        &user.address
                    );
                }
            }
            //
            self.user_connections.insert(user.address, new_connection);
            if self.io.bandwidth_monitor_enabled() {
                self.io.register_client(&user.address);
            }
            self.incoming_events.push_connection(user_key);
        }
    }

    /// Rejects an incoming Client User, terminating their attempt to establish
    /// a connection with the Server
    pub fn reject_connection(&mut self, user_key: &UserKey) {
        if let Some(user) = self.users.get(user_key) {
            // send connect reject response
            let mut writer = self.handshake_manager.write_reject_response();
            match self.io.send_writer(&user.address, &mut writer) {
                Ok(()) => {}
                Err(_) => {
                    // TODO: pass this on and handle above
                    warn!(
                        "Server Error: Cannot send auth rejection packet to {}",
                        &user.address
                    );
                }
            }
            //
        }
        self.user_delete(user_key);
    }

    // Messages

    /// Queues up an Message to be sent to the Client associated with a given
    /// UserKey
    pub fn send_message<C: Channel, M: Message>(&mut self, user_key: &UserKey, message: &M) {
        let cloned_message = M::clone_box(message);
        self.send_message_inner(user_key, &Channels::type_to_id::<C>(), cloned_message);
    }

    /// Queues up an Message to be sent to the Client associated with a given
    /// UserKey
    fn send_message_inner(
        &mut self,
        user_key: &UserKey,
        channel_id: &ChannelId,
        message: Box<dyn Message>,
    ) {
        if !Channels::channel(channel_id).can_send_to_client() {
            panic!("Cannot send message to Client on this Channel");
        }

        if let Some(user) = self.users.get(user_key) {
            if let Some(connection) = self.user_connections.get_mut(&user.address) {
                if message.has_entity_properties() {
                    // collect all entities in the message
                    let entities: Vec<Entity> = message
                        .entities()
                        .iter()
                        .map(|handle| self.world_record.handle_to_entity(handle))
                        .collect();

                    // check whether all entities are in scope for the connection
                    let all_entities_in_scope = {
                        entities
                            .iter()
                            .all(|entity| connection.entity_manager.entity_channel_is_open(entity))
                    };
                    if all_entities_in_scope {
                        // All necessary entities are in scope, so send message
                        connection
                            .base
                            .message_manager
                            .send_message(channel_id, message);
                    } else {
                        // Entity hasn't been added to the User Scope yet, or replicated to Client
                        // yet
                        connection
                            .entity_manager
                            .queue_entity_message(entities, channel_id, message);
                    }
                } else {
                    connection
                        .base
                        .message_manager
                        .send_message(channel_id, message);
                }
            }
        }
    }

    /// Sends a message to all connected users using a given channel
    pub fn broadcast_message<C: Channel, M: Message>(&mut self, message: M) {
        self.broadcast_message_inner(&Channels::type_to_id::<C>(), Box::new(message));
    }

    fn broadcast_message_inner(&mut self, channel_id: &ChannelId, message: Box<dyn Message>) {
        self.user_keys()
            .iter()
            .for_each(|user_key| self.send_message_inner(user_key, channel_id, message.clone()))
    }

    // Updates

    /// Used to evaluate whether, given a User & Entity that are in the
    /// same Room, said Entity should be in scope for the given User.
    ///
    /// While Rooms allow for a very simple scope to which an Entity can belong,
    /// this provides complete customization for advanced scopes.
    ///
    /// Return a collection of Entity Scope Sets, being a unique combination of
    /// a related Room, User, and Entity, used to determine which Entities to
    /// replicate to which Users
    pub fn scope_checks(&self) -> Vec<(RoomKey, UserKey, Entity)> {
        let mut list: Vec<(RoomKey, UserKey, Entity)> = Vec::new();

        // TODO: precache this, instead of generating a new list every call
        // likely this is called A LOT
        for (room_key, room) in self.rooms.iter() {
            for user_key in room.user_keys() {
                for entity in room.entities() {
                    list.push((room_key, *user_key, *entity));
                }
            }
        }

        list
    }

    /// Sends all update messages to all Clients. If you don't call this
    /// method, the Server will never communicate with it's connected
    /// Clients
    pub fn send_all_updates(&mut self, world: World) {
        let now = Instant::now();

        // update entity scopes
        self.update_entity_scopes(&world);

        // loop through all connections, send packet
        let mut user_addresses: Vec<SocketAddr> = self.user_connections.keys().copied().collect();
        fastrand::shuffle(&mut user_addresses);

        for user_address in user_addresses {
            let connection = self.user_connections.get_mut(&user_address).unwrap();

            let rtt = connection.ping_manager.rtt;

            connection.send_outgoing_packets(
                &now,
                &mut self.io,
                &world,
                &self.tick_manager,
                &rtt,
            );
        }
    }


    // Users

    /// Returns whether or not a User exists for the given RoomKey
    pub fn user_exists(&self, user_key: &UserKey) -> bool {
        self.users.contains_key(user_key)
    }

    /// Retrieves an UserRef that exposes read-only operations for the User
    /// associated with the given UserKey.
    /// Panics if the user does not exist.
    pub fn user(&self, user_key: &UserKey) -> UserRef {
        if self.users.contains_key(user_key) {
            return UserRef::new(self, user_key);
        }
        panic!("No User exists for given Key!");
    }

    /// Retrieves an UserMut that exposes read and write operations for the User
    /// associated with the given UserKey.
    /// Returns None if the user does not exist.
    pub fn user_mut(&mut self, user_key: &UserKey) -> UserMut {
        if self.users.contains_key(user_key) {
            return UserMut::new(self, user_key);
        }
        panic!("No User exists for given Key!");
    }

    /// Return a list of all currently connected Users' keys
    pub fn user_keys(&self) -> Vec<UserKey> {
        let mut output = Vec::new();

        for (user_key, _) in self.users.iter() {
            output.push(user_key);
        }

        output
    }

    /// Get the number of Users currently connected
    pub fn users_count(&self) -> usize {
        self.users.len()
    }

    /// Returns a UserScopeMut, which is used to include/exclude Entities for a
    /// given User
    pub fn user_scope(&mut self, user_key: &UserKey) -> UserScopeMut {
        if self.users.contains_key(user_key) {
            return UserScopeMut::new(self, user_key);
        }
        panic!("No User exists for given Key!");
    }

    // Rooms

    /// Creates a new Room on the Server and returns a corresponding RoomMut,
    /// which can be used to add users/entities to the room or retrieve its
    /// key
    pub fn make_room(&mut self) -> RoomMut {
        let new_room = Room::new();
        let room_key = self.rooms.insert(new_room);
        RoomMut::new(self, &room_key)
    }

    /// Returns whether or not a Room exists for the given RoomKey
    pub fn room_exists(&self, room_key: &RoomKey) -> bool {
        self.rooms.contains_key(room_key)
    }

    /// Retrieves an RoomMut that exposes read and write operations for the
    /// Room associated with the given RoomKey.
    /// Panics if the room does not exist.
    pub fn room(&self, room_key: &RoomKey) -> RoomRef {
        if self.rooms.contains_key(room_key) {
            return RoomRef::new(self, room_key);
        }
        panic!("No Room exists for given Key!");
    }

    /// Retrieves an RoomMut that exposes read and write operations for the
    /// Room associated with the given RoomKey.
    /// Panics if the room does not exist.
    pub fn room_mut(&mut self, room_key: &RoomKey) -> RoomMut {
        if self.rooms.contains_key(room_key) {
            return RoomMut::new(self, room_key);
        }
        panic!("No Room exists for given Key!");
    }

    /// Return a list of all the Server's Rooms' keys
    pub fn room_keys(&self) -> Vec<RoomKey> {
        let mut output = Vec::new();

        for (key, _) in self.rooms.iter() {
            output.push(key);
        }

        output
    }

    /// Get a count of how many Rooms currently exist
    pub fn rooms_count(&self) -> usize {
        self.rooms.len()
    }

    // Ticks

    /// Gets the last received tick from the Client
    pub fn client_tick(&self, user_key: &UserKey) -> Option<Tick> {
        if let Some(user) = self.users.get(user_key) {
            if let Some(user_connection) = self.user_connections.get(&user.address) {
                return Some(user_connection.last_received_tick);
            }
        }
        None
    }

    /// Gets the current tick of the Server
    pub fn server_tick(&self) -> Option<Tick> {
        return self
            .tick_manager
            .as_ref()
            .map(|tick_manager| tick_manager.server_tick());
    }

    // Bandwidth monitoring
    pub fn outgoing_bandwidth_total(&mut self) -> f32 {
        self.io.outgoing_bandwidth_total()
    }

    pub fn incoming_bandwidth_total(&mut self) -> f32 {
        self.io.incoming_bandwidth_total()
    }

    pub fn outgoing_bandwidth_to_client(&mut self, address: &SocketAddr) -> f32 {
        self.io.outgoing_bandwidth_to_client(address)
    }

    pub fn incoming_bandwidth_from_client(&mut self, address: &SocketAddr) -> f32 {
        self.io.incoming_bandwidth_from_client(address)
    }

    // Ping
    /// Gets the average Round Trip Time measured to the given User's Client
    pub fn rtt(&self, user_key: &UserKey) -> Option<f32> {
        if let Some(user) = self.users.get(user_key) {
            if let Some(user_connection) = self.user_connections.get(&user.address) {
                return Some(user_connection.ping_manager.rtt);
            }
        }
        None
    }

    /// Gets the average Jitter measured in connection to the given User's
    /// Client
    pub fn jitter(&self, user_key: &UserKey) -> Option<f32> {
        if let Some(user) = self.users.get(user_key) {
            if let Some(user_connection) = self.user_connections.get(&user.address) {
                return Some(user_connection.ping_manager.jitter);
            }
        }
        None
    }

    // Crate-Public methods

    //// Entities

    /// Despawns the Entity, if it exists.
    /// This will also remove all of the Entity’s Components.
    /// Returns true if the Entity is successfully despawned and false if the
    /// Entity does not exist.
    pub(crate) fn despawn_entity(&mut self, world: &mut World, entity: &Entity) {
        if !world.has_entity(entity) {
            panic!("attempted to de-spawn nonexistent entity");
        }

        // TODO: we can make this more efficient in the future by caching which Entities
        // are in each User's scope
        for (_, user_connection) in self.user_connections.iter_mut() {
            //remove entity from user connection
            user_connection.entity_manager.despawn_entity(entity);
        }

        // Clean up associated components
        for component_kind in self.world_record.component_kinds(entity).unwrap() {
            self.component_cleanup(entity, &component_kind);
        }

        // Delete from world
        world.despawn_entity(entity);

        // Delete scope
        self.entity_scope_map.remove_entity(entity);
    }

    //// Entity Scopes

    /// Remove all entities from a User's scope
    pub(crate) fn user_scope_remove_user(&mut self, user_key: &UserKey) {
        self.entity_scope_map.remove_user(user_key);
    }

    pub(crate) fn user_scope_set_entity(
        &mut self,
        user_key: &UserKey,
        entity: &Entity,
        is_contained: bool,
    ) {
        self.entity_scope_map
            .insert(*user_key, *entity, is_contained);
    }

    //// Components

    /// Adds a Component to an Entity
    pub(crate) fn insert_component<C: Component>(
        &mut self,
        world: &mut World,
        entity: &Entity,
        component: C,
    ) {
        if !world.has_entity(entity) {
            panic!("attempted to add component to non-existent entity");
        }

        if world.entity(*entity).contains::<C>() {
            // Entity already has this Component type, update Component
            world.entity_mut(*entity).insert(component);
            // TODO: shouldn't i replicate this insert_component?
        } else {
            // actually insert component into world
            world.entity_mut(*entity).insert(component);

            // add component to connections already tracking entity
            for (_, user_connection) in self.user_connections.iter_mut() {
                // insert component into user's connection
                if user_connection.entity_manager.scope_has_entity(entity) {
                    user_connection
                        .entity_manager
                        .insert_component(entity, &component_kind);
                }
            }
        }
    }

    /// Removes a Component from an Entity
    pub(crate) fn remove_component<R: Replicate>(
        &mut self,
        world: &mut World,
        entity: &Entity,
    ) -> Option<R> {
        // get component key from type
        let component_id = Components::type_to_id::<R>();

        // clean up component on all connections

        // TODO: should be able to make this more efficient by caching for every Entity
        // which scopes they are part of
        for (_, user_connection) in self.user_connections.iter_mut() {
            // remove component from user connection
            user_connection
                .entity_manager
                .remove_component(entity, &component_id);
        }

        // cleanup all other loose ends
        self.component_cleanup(entity, &component_id);

        // remove from world
        world.remove_component::<R>(entity)
    }

    //// Users

    /// Get a User's Socket Address, given the associated UserKey
    pub(crate) fn user_address(&self, user_key: &UserKey) -> Option<SocketAddr> {
        if let Some(user) = self.users.get(user_key) {
            return Some(user.address);
        }
        None
    }

    /// Returns an iterator of all the keys of the [`Room`]s the User belongs to
    pub(crate) fn user_room_keys(&self, user_key: &UserKey) -> Option<Iter<RoomKey>> {
        if let Some(user) = self.users.get(user_key) {
            return Some(user.room_keys());
        }
        return None;
    }

    /// Get an count of how many Rooms the given User is inside
    pub(crate) fn user_rooms_count(&self, user_key: &UserKey) -> Option<usize> {
        if let Some(user) = self.users.get(user_key) {
            return Some(user.room_count());
        }
        return None;
    }

    pub(crate) fn user_disconnect(&mut self, user_key: &UserKey) {
        if let Some(user) = self.user_delete(user_key) {
            self.incoming_events.push_disconnection(user_key, user);
        }
    }

    /// All necessary cleanup, when they're actually gone...
    pub(crate) fn user_delete(&mut self, user_key: &UserKey) -> Option<User> {
        if let Some(user) = self.users.remove(user_key) {
            if self.user_connections.remove(&user.address).is_some() {
                self.entity_scope_map.remove_user(user_key);
                self.handshake_manager.delete_user(&user.address);

                // Clean up all user data
                for room_key in user.room_keys() {
                    self.rooms
                        .get_mut(room_key)
                        .unwrap()
                        .unsubscribe_user(user_key);
                }

                if self.io.bandwidth_monitor_enabled() {
                    self.io.deregister_client(&user.address);
                }

                return Some(user);
            }
        }

        None
    }

    //// Rooms

    /// Deletes the Room associated with a given RoomKey on the Server.
    /// Returns true if the Room existed.
    pub(crate) fn room_destroy(&mut self, room_key: &RoomKey) -> bool {
        self.room_remove_all_entities(room_key);

        if self.rooms.contains_key(room_key) {
            // TODO: what else kind of cleanup do we need to do here? Scopes?

            // actually remove the room from the collection
            let room = self.rooms.remove(room_key).unwrap();
            for user_key in room.user_keys() {
                self.users.get_mut(user_key).unwrap().uncache_room(room_key);
            }

            true
        } else {
            false
        }
    }

    //////// users

    /// Returns whether or not an User is currently in a specific Room, given
    /// their keys.
    pub(crate) fn room_has_user(&self, room_key: &RoomKey, user_key: &UserKey) -> bool {
        if let Some(room) = self.rooms.get(room_key) {
            return room.has_user(user_key);
        }
        false
    }

    /// Add an User to a Room, given the appropriate RoomKey & UserKey
    /// Entities will only ever be in-scope for Users which are in a
    /// Room with them
    pub(crate) fn room_add_user(&mut self, room_key: &RoomKey, user_key: &UserKey) {
        if let Some(user) = self.users.get_mut(user_key) {
            if let Some(room) = self.rooms.get_mut(room_key) {
                room.subscribe_user(user_key);
                user.cache_room(room_key);
            }
        }
    }

    /// Removes a User from a Room
    pub(crate) fn room_remove_user(&mut self, room_key: &RoomKey, user_key: &UserKey) {
        if let Some(user) = self.users.get_mut(user_key) {
            if let Some(room) = self.rooms.get_mut(room_key) {
                room.unsubscribe_user(user_key);
                user.uncache_room(room_key);
            }
        }
    }

    /// Get a count of Users in a given Room
    pub(crate) fn room_users_count(&self, room_key: &RoomKey) -> usize {
        if let Some(room) = self.rooms.get(room_key) {
            return room.users_count();
        }
        0
    }

    /// Returns an iterator of the [`UserKey`] for Users that belong in the Room
    pub(crate) fn room_user_keys(&self, room_key: &RoomKey) -> impl Iterator<Item = &UserKey> {
        let iter = if let Some(room) = self.rooms.get(room_key) {
            Some(room.user_keys())
        } else {
            None
        };
        iter.into_iter().flatten()
    }

    /// Sends a message to all connected users in a given Room using a given channel
    pub(crate) fn room_broadcast_message(
        &mut self,
        channel_id: &ChannelId,
        message: Box<dyn Message>,
        room_key: &RoomKey,
    ) {
        if let Some(room) = self.rooms.get(room_key) {
            let user_keys: Vec<UserKey> = room.user_keys().cloned().collect();
            for user_key in &user_keys {
                self.send_message_inner(user_key, channel_id, message.clone())
            }
        }
    }

    //////// entities

    /// Returns whether or not an Entity is currently in a specific Room, given
    /// their keys.
    pub(crate) fn room_has_entity(&self, room_key: &RoomKey, entity: &Entity) -> bool {
        self.world_record.entity_is_in_room(entity, room_key)
    }

    /// Add an Entity to a Room associated with the given RoomKey.
    /// Entities will only ever be in-scope for Users which are in a Room with
    /// them.
    pub(crate) fn room_add_entity(&mut self, room_key: &RoomKey, entity: &Entity) {
        let mut is_some = false;
        if let Some(room) = self.rooms.get_mut(room_key) {
            room.add_entity(entity);
            is_some = true;
        }
        if is_some {
            self.world_record.entity_enter_room(entity, room_key);
        }
    }

    /// Remove an Entity from a Room, associated with the given RoomKey
    pub(crate) fn room_remove_entity(&mut self, room_key: &RoomKey, entity: &Entity) {
        if let Some(room) = self.rooms.get_mut(room_key) {
            room.remove_entity(entity);
            self.world_record.entity_leave_rooms(entity);
        }
    }

    /// Remove all Entities from a Room, associated with the given RoomKey
    fn room_remove_all_entities(&mut self, room_key: &RoomKey) {
        if let Some(room) = self.rooms.get_mut(room_key) {
            let entities: Vec<Entity> = room.entities().copied().collect();
            for entity in entities {
                room.remove_entity(&entity);
                self.world_record.entity_leave_rooms(&entity);
            }
        }
    }

    /// Get a count of Entities in a given Room
    pub(crate) fn room_entities_count(&self, room_key: &RoomKey) -> usize {
        if let Some(room) = self.rooms.get(room_key) {
            return room.entities_count();
        }
        0
    }

    // Private methods

    /// Maintain connection with a client and read all incoming packet data
    fn maintain_socket(&mut self) {
        // disconnects
        if self.timeout_timer.ringing() {
            self.timeout_timer.reset();

            let mut user_disconnects: Vec<UserKey> = Vec::new();

            for (_, connection) in &mut self.user_connections.iter_mut() {
                // user disconnects
                if connection.base.should_drop() {
                    user_disconnects.push(connection.user_key);
                    continue;
                }
            }

            for user_key in user_disconnects {
                self.user_disconnect(&user_key);
            }
        }

        // heartbeats
        if self.heartbeat_timer.ringing() {
            self.heartbeat_timer.reset();

            for (user_address, connection) in &mut self.user_connections.iter_mut() {
                // user heartbeats
                if connection.base.should_send_heartbeat() {
                    // Don't try to refactor this to self.internal_send, doesn't seem to
                    // work cause of iter_mut()
                    let mut writer = BitWriter::new();

                    // write header
                    connection
                        .base
                        .write_outgoing_header(PacketType::Heartbeat, &mut writer);

                    // write server tick
                    if let Some(tick_manager) = self.tick_manager.as_mut() {
                        tick_manager.write_server_tick(&mut writer);
                    }

                    // send packet
                    match self.io.send_writer(user_address, &mut writer) {
                        Ok(()) => {}
                        Err(_) => {
                            // TODO: pass this on and handle above
                            warn!(
                                "Server Error: Cannot send heartbeat packet to {}",
                                user_address
                            );
                        }
                    }
                    connection.base.mark_sent();
                }
            }
        }

        // pings
        if self.ping_timer.ringing() {
            self.ping_timer.reset();

            for (user_address, connection) in &mut self.user_connections.iter_mut() {
                // send pings
                if connection.ping_manager.should_send_ping() {
                    let mut writer = BitWriter::new();

                    // write header
                    connection
                        .base
                        .write_outgoing_header(PacketType::Ping, &mut writer);

                    // write client tick
                    if let Some(tick_manager) = self.tick_manager.as_mut() {
                        tick_manager.write_server_tick(&mut writer);
                    }

                    // write body
                    connection.ping_manager.write_ping(&mut writer);

                    // send packet
                    match self.io.send_writer(user_address, &mut writer) {
                        Ok(()) => {}
                        Err(_) => {
                            // TODO: pass this on and handle above
                            warn!("Server Error: Cannot send ping packet to {}", user_address);
                        }
                    }
                    connection.base.mark_sent();
                }
            }
        }

        //receive socket events
        loop {
            match self.io.recv_reader() {
                Ok(Some((address, owned_reader))) => {
                    let mut reader = owned_reader.borrow();

                    // Read header
                    let header_result = StandardHeader::de(&mut reader);
                    if header_result.is_err() {
                        // Received a malformed packet
                        // TODO: increase suspicion against packet sender
                        continue;
                    }
                    let header = header_result.unwrap();

                    // Handshake stuff
                    match header.packet_type {
                        PacketType::ClientChallengeRequest => {
                            if let Ok(mut writer) =
                                self.handshake_manager.recv_challenge_request(&mut reader)
                            {
                                match self.io.send_writer(&address, &mut writer) {
                                    Ok(()) => {}
                                    Err(_) => {
                                        // TODO: pass this on and handle above
                                        warn!("Server Error: Cannot send challenge response packet to {}", &address);
                                    }
                                };
                            }
                            continue;
                        }
                        PacketType::ClientConnectRequest => {
                            match self
                                .handshake_manager
                                .recv_connect_request(&address, &mut reader)
                            {
                                HandshakeResult::Success(auth_message_opt) => {
                                    if self.user_connections.contains_key(&address) {
                                        // send connectaccept response
                                        let mut writer =
                                            self.handshake_manager.write_connect_response();
                                        match self.io.send_writer(&address, &mut writer) {
                                            Ok(()) => {}
                                            Err(_) => {
                                                // TODO: pass this on and handle above
                                                warn!("Server Error: Cannot send connect success response packet to {}", &address);
                                            }
                                        };
                                        //
                                    } else {
                                        let user = User::new(address);
                                        let user_key = self.users.insert(user);

                                        if let Some(auth_message) = auth_message_opt {
                                            self.incoming_events.push_auth(&user_key, auth_message);
                                        } else {
                                            self.accept_connection(&user_key);
                                        }
                                    }
                                }
                                HandshakeResult::Invalid => {
                                    // do nothing
                                }
                            }
                            continue;
                        }
                        _ => {}
                    }

                    // Packets requiring established connection
                    if let Some(user_connection) = self.user_connections.get_mut(&address) {
                        // Mark that we've heard from the client
                        user_connection.base.mark_heard();

                        // Process incoming header
                        user_connection.process_incoming_header(&header);

                        match header.packet_type {
                            PacketType::Data => {
                                // read client tick
                                let server_and_client_tick_opt = {
                                    if let Some(tick_manager) = self.tick_manager.as_ref() {
                                        ////
                                        let client_tick_result =
                                            tick_manager.read_client_tick(&mut reader);
                                        if client_tick_result.is_err() {
                                            // Received a malformed packet
                                            // TODO: increase suspicion against packet sender
                                            continue;
                                        }
                                        let client_tick = client_tick_result.unwrap();
                                        user_connection.recv_client_tick(client_tick);
                                        ////

                                        let server_tick = tick_manager.server_tick();

                                        Some((server_tick, client_tick))
                                    } else {
                                        None
                                    }
                                };

                                // process data
                                let data_result = user_connection.process_incoming_data(
                                    server_and_client_tick_opt,
                                    &mut reader,
                                    &self.world_record,
                                );
                                if data_result.is_err() {
                                    // Received a malformed packet
                                    // TODO: increase suspicion against packet sender
                                    warn!("Error reading incoming packet!");
                                    continue;
                                }
                            }
                            PacketType::Disconnect => {
                                if self
                                    .handshake_manager
                                    .verify_disconnect_request(user_connection, &mut reader)
                                {
                                    let user_key = user_connection.user_key;
                                    self.user_disconnect(&user_key);
                                }
                            }
                            PacketType::Heartbeat => {
                                // read client tick, don't need to do anything else
                                if let Some(tick_manager) = self.tick_manager.as_ref() {
                                    ////
                                    let client_tick_result =
                                        tick_manager.read_client_tick(&mut reader);
                                    if client_tick_result.is_err() {
                                        // Received a malformed packet
                                        // TODO: increase suspicion against packet sender
                                        continue;
                                    }
                                    let client_tick = client_tick_result.unwrap();
                                    user_connection.recv_client_tick(client_tick);
                                    ////
                                }
                            }
                            PacketType::Ping => {
                                // read client tick
                                if let Some(tick_manager) = self.tick_manager.as_ref() {
                                    ////
                                    let client_tick_result =
                                        tick_manager.read_client_tick(&mut reader);
                                    if client_tick_result.is_err() {
                                        // Received a malformed packet
                                        // TODO: increase suspicion against packet sender
                                        continue;
                                    }
                                    let client_tick = client_tick_result.unwrap();
                                    user_connection.recv_client_tick(client_tick);
                                    ////
                                }

                                // read incoming ping index
                                let ping_index = u16::de(&mut reader).unwrap();

                                // write pong payload
                                let mut writer = BitWriter::new();

                                // write header
                                user_connection
                                    .base
                                    .write_outgoing_header(PacketType::Pong, &mut writer);

                                // write server tick
                                if let Some(tick_manager) = self.tick_manager.as_ref() {
                                    tick_manager.write_server_tick(&mut writer);
                                }

                                // write index
                                ping_index.ser(&mut writer);

                                // send packet
                                match self.io.send_writer(&address, &mut writer) {
                                    Ok(()) => {}
                                    Err(_) => {
                                        // TODO: pass this on and handle above
                                        warn!(
                                            "Server Error: Cannot send pong packet to {}",
                                            &address
                                        );
                                    }
                                };
                                user_connection.base.mark_sent();
                            }
                            PacketType::Pong => {
                                // read client tick
                                if let Some(tick_manager) = self.tick_manager.as_ref() {
                                    ////
                                    let client_tick_result =
                                        tick_manager.read_client_tick(&mut reader);
                                    if client_tick_result.is_err() {
                                        // Received a malformed packet
                                        // TODO: increase suspicion against packet sender
                                        continue;
                                    }
                                    let client_tick = client_tick_result.unwrap();
                                    user_connection.recv_client_tick(client_tick);
                                    ////
                                }

                                // TODO: send a message to client with a recommendation on how
                                //  to speedup/slowdown simulation?
                                user_connection.ping_manager.process_pong(&mut reader);
                            }
                            _ => {}
                        }
                    }
                }
                Ok(NonEntity) => {
                    // No more packets, break loop
                    break;
                }
                Err(error) => {
                    self.incoming_events
                        .push_error(NaiaServerError::Wrapped(Box::new(error)));
                }
            }
        }
    }

    // Entity Helpers

    fn spawn_entity_init(&mut self, entity: &Entity) {
        self.world_record.spawn_entity(entity);
    }

    // Entity Scopes

    fn update_entity_scopes(&mut self, world: &World) {
        for (_, room) in self.rooms.iter_mut() {
            while let Some((removed_user, removed_entity)) = room.pop_entity_removal_queue() {
                if let Some(user) = self.users.get(&removed_user) {
                    if let Some(user_connection) = self.user_connections.get_mut(&user.address) {
                        // TODO: evaluate whether the Entity really needs to be despawned!
                        // What if the Entity shares another Room with this User? It shouldn't be despawned!

                        //remove entity from user connection
                        user_connection
                            .entity_manager
                            .despawn_entity(&removed_entity);
                    }
                }
            }

            // TODO: we should be able to cache these tuples of keys to avoid building a new
            // list each time
            for user_key in room.user_keys() {
                for entity in room.entities() {
                    if world.has_entity(entity) {
                        if let Some(user) = self.users.get(user_key) {
                            if let Some(user_connection) =
                                self.user_connections.get_mut(&user.address)
                            {
                                let currently_in_scope =
                                    user_connection.entity_manager.scope_has_entity(entity);

                                let should_be_in_scope = if let Some(in_scope) =
                                    self.entity_scope_map.get(user_key, entity)
                                {
                                    *in_scope
                                } else {
                                    false
                                };

                                if should_be_in_scope {
                                    if !currently_in_scope {
                                        // add entity to the connections local scope
                                        user_connection.entity_manager.spawn_entity(entity);
                                        // add components to connections local scope
                                        for component_kind in
                                            self.world_record.component_kinds(entity).unwrap()
                                        {
                                            user_connection
                                                .entity_manager
                                                .insert_component(entity, &component_kind);
                                        }
                                    }
                                } else if currently_in_scope {
                                    // remove entity from the connections local scope
                                    user_connection.entity_manager.despawn_entity(entity);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

}