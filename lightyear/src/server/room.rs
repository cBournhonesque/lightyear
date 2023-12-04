use crate::netcode::ClientId;
use crate::prelude::{MainSet, Replicate};
use crate::protocol::Protocol;
use crate::server::resource::Server;
use crate::server::systems::is_ready_to_send;
use crate::shared::replication::components::DespawnTracker;
use crate::utils::wrapping_id::wrapping_id;
use bevy::app::App;
use bevy::prelude::{
    Entity, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PostUpdate, Query, RemovedComponents,
    Res, ResMut, Resource, SystemSet,
};
use std::collections::{HashMap, HashSet};

// Id for a room, used to perform interest management
// An entity will be replicated to a client only if they are in the same room
wrapping_id!(RoomId);

/// Resource that will track any changes in the rooms
/// (we cannot use bevy `Events` directly because we don't need to send this every frame.
/// Also, we only need to keep track of updates for each send_interval frame. That means that if an entity
/// leaves then re-joins a room within the same send_interval period, we don't need to send any update)
///
/// This will be cleared every time the Server sends updates to the Client (every send_interval)
#[derive(Resource)]
pub struct RoomEvents {
    client_enter_room: HashMap<ClientId, HashSet<RoomId>>,
    client_leave_room: HashMap<ClientId, HashSet<RoomId>>,
    entity_enter_room: HashMap<Entity, HashSet<RoomId>>,
    entity_leave_room: HashMap<Entity, HashSet<RoomId>>,
}

impl Default for RoomEvents {
    fn default() -> Self {
        Self {
            client_enter_room: HashMap::new(),
            client_leave_room: HashMap::new(),
            entity_enter_room: HashMap::new(),
            entity_leave_room: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub struct RoomData {
    client_to_rooms: HashMap<ClientId, HashSet<RoomId>>,
    entity_to_rooms: HashMap<Entity, HashSet<RoomId>>,
    rooms: HashMap<RoomId, Room>,
}

#[derive(Default)]
pub struct Room {
    /// list of clients that are in the room
    clients: HashSet<ClientId>,
    /// list of entities that are in the room
    entities: HashSet<Entity>,
}

#[derive(Default)]
pub struct RoomManager {
    events: RoomEvents,
    data: RoomData,
}

#[derive(Default)]
pub struct RoomPlugin<P: Protocol> {
    _marker: std::marker::PhantomData<P>,
}

/// System sets related to Rooms
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomSystemSets {
    /// Use all the room events that happened, and use those to update
    /// the replication caches
    UpdateReplicationCaches,
    /// Perform bookkeeping for the rooms
    /// (remove despawned entities, update the replication caches, etc.)
    RoomBookkeeping,
}

impl<P: Protocol> Plugin for RoomPlugin<P> {
    fn build(&self, app: &mut App) {
        // SETS
        app.configure_sets(
            PostUpdate,
            (
                RoomSystemSets::UpdateReplicationCaches,
                MainSet::Send,
                RoomSystemSets::RoomBookkeeping,
            )
                .chain()
                .run_if(is_ready_to_send::<P>),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            (
                update_entity_replication_cache::<P>
                    .in_set(RoomSystemSets::UpdateReplicationCaches),
                (
                    clear_entity_replication_cache::<P>,
                    clean_entity_despawns::<P>,
                )
                    .in_set(RoomSystemSets::RoomBookkeeping),
            ),
        );
    }
}

impl RoomManager {
    /// Remove the client from all the rooms it was in
    pub(crate) fn client_disconnect(&mut self, client_id: ClientId) {
        if let Some(rooms) = self.data.client_to_rooms.remove(&client_id) {
            for room_id in rooms {
                RoomMut::new(self, room_id).remove_client(client_id);
                self.remove_client(room_id, client_id);
            }
        }
    }

    /// Remove the entity from all the rooms it was in
    pub(crate) fn entity_despawn(&mut self, entity: Entity) {
        if let Some(rooms) = self.data.entity_to_rooms.remove(&entity) {
            for room_id in rooms {
                RoomMut::new(self, room_id).remove_entity(entity);
                self.remove_entity(room_id, entity);
            }
        }
    }

    fn add_client(&mut self, room_id: RoomId, client_id: ClientId) {
        self.data
            .client_to_rooms
            .entry(client_id)
            .or_default()
            .insert(room_id);
        self.data
            .rooms
            .entry(room_id)
            .or_default()
            .clients
            .insert(client_id);
        self.events.client_enter_room(room_id, client_id);
    }

    fn remove_client(&mut self, room_id: RoomId, client_id: ClientId) {
        self.data
            .client_to_rooms
            .entry(client_id)
            .or_default()
            .remove(&room_id);
        self.data
            .rooms
            .entry(room_id)
            .or_default()
            .clients
            .remove(&client_id);
        self.events.client_leave_room(room_id, client_id);
    }

    fn add_entity(&mut self, room_id: RoomId, entity: Entity) {
        self.data
            .entity_to_rooms
            .entry(entity)
            .or_default()
            .insert(room_id);
        self.data
            .rooms
            .entry(room_id)
            .or_default()
            .entities
            .insert(entity);
        self.events.entity_enter_room(room_id, entity);
    }

    fn remove_entity(&mut self, room_id: RoomId, entity: Entity) {
        self.data
            .entity_to_rooms
            .entry(entity)
            .or_default()
            .remove(&room_id);
        self.data
            .rooms
            .entry(room_id)
            .or_default()
            .entities
            .remove(&entity);
        self.events.entity_leave_room(room_id, entity);
    }
}

/// Convenient wrapper to mutate a room
pub struct RoomMut<'s> {
    pub(crate) id: RoomId,
    pub(crate) manager: &'s mut RoomManager,
}

impl<'s> RoomMut<'s> {
    fn new(manager: &'s mut RoomManager, id: RoomId) -> Self {
        Self { id, manager }
    }

    pub fn add_client(&mut self, client_id: ClientId) {
        self.manager.add_client(self.id, client_id)
    }

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.manager.remove_client(self.id, client_id)
    }

    pub fn add_entity(&mut self, entity: Entity) {
        self.manager.add_entity(self.id, entity)
    }

    pub fn remove_entity(&mut self, entity: Entity) {
        self.manager.remove_entity(self.id, entity)
    }
}

impl RoomEvents {
    /// A client joined a room
    pub fn client_enter_room(&mut self, room_id: RoomId, client_id: ClientId) {
        // if the client had left the room, no need to track the enter
        if !self
            .client_leave_room
            .entry(client_id)
            .or_default()
            .remove(&room_id)
        {
            self.client_enter_room
                .entry(client_id)
                .or_default()
                .insert(room_id);
        }
    }

    pub fn client_leave_room(&mut self, room_id: RoomId, client_id: ClientId) {
        if !self
            .client_enter_room
            .entry(client_id)
            .or_default()
            .remove(&room_id)
        {
            self.client_leave_room
                .entry(client_id)
                .or_default()
                .insert(room_id);
        }
    }

    pub fn entity_enter_room(&mut self, room_id: RoomId, entity: Entity) {
        if !self
            .entity_leave_room
            .entry(entity)
            .or_default()
            .remove(&room_id)
        {
            self.entity_enter_room
                .entry(entity)
                .or_default()
                .insert(room_id);
        }
    }

    pub fn entity_leave_room(&mut self, room_id: RoomId, entity: Entity) {
        if !self
            .entity_enter_room
            .entry(entity)
            .or_default()
            .remove(&room_id)
        {
            self.entity_leave_room
                .entry(entity)
                .or_default()
                .insert(room_id);
        }
    }

    fn iter_client_enter_room(&self) -> impl Iterator<Item = (&ClientId, &HashSet<RoomId>)> {
        self.client_enter_room.iter()
    }

    fn iter_client_leave_room(&self) -> impl Iterator<Item = (&ClientId, &HashSet<RoomId>)> {
        self.client_leave_room.iter()
    }

    fn iter_entity_enter_room(&self) -> impl Iterator<Item = (&Entity, &HashSet<RoomId>)> {
        self.entity_enter_room.iter()
    }

    fn iter_entity_leave_room(&self) -> impl Iterator<Item = (&Entity, &HashSet<RoomId>)> {
        self.entity_leave_room.iter()
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum ClientVisibility {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
}

/// Update each entities' replication-client-list based on the room events
/// Note that the rooms' entities/clients have already been updated at this point
fn update_entity_replication_cache<P: Protocol>(
    server: Res<Server<P>>,
    mut query: Query<&mut Replicate>,
) {
    // entity joined room
    for (entity, rooms) in server.room_manager.events.iter_entity_enter_room() {
        // for each room joined, update the entity's client visibility list
        rooms.iter().for_each(|room_id| {
            let room = server.room_manager.data.rooms.get(room_id).unwrap();
            room.clients.iter().for_each(|client_id| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    // only set it to gained if it wasn't present before
                    replicate
                        .replication_clients_cache
                        .entry(*client_id)
                        .or_insert(ClientVisibility::Gained);
                }
            });
        });
    }
    // entity left room
    for (entity, rooms) in server.room_manager.events.iter_entity_leave_room() {
        // for each room left, update the entity's client visibility list
        rooms.iter().for_each(|room_id| {
            let room = server.room_manager.data.rooms.get(room_id).unwrap();
            room.clients.iter().for_each(|client_id| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    replicate
                        .replication_clients_cache
                        .insert(*client_id, ClientVisibility::Lost);
                }
            });
        });
    }
    // client joined room: update all the entities that are in that room
    for (client_id, rooms) in server.room_manager.events.iter_client_enter_room() {
        rooms.iter().for_each(|room_id| {
            let room = server.room_manager.data.rooms.get(room_id).unwrap();
            room.entities.iter().for_each(|entity| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    replicate
                        .replication_clients_cache
                        .entry(*client_id)
                        .or_insert(ClientVisibility::Gained);
                }
            });
        });
    }
    // client left room: update all the entities that are in that room
    for (client_id, rooms) in server.room_manager.events.iter_client_leave_room() {
        rooms.iter().for_each(|room_id| {
            let room = server.room_manager.data.rooms.get(room_id).unwrap();
            room.entities.iter().for_each(|entity| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    replicate
                        .replication_clients_cache
                        .insert(*client_id, ClientVisibility::Lost);
                }
            });
        });
    }
}

/// After replication, update the Replication Cache:
/// - Visibility Gained becomes Visibility Maintained
/// - Visibility Lost gets removed from the cache
fn clear_entity_replication_cache<P: Protocol>(mut query: Query<&mut Replicate>) {
    for mut replicate in query.iter_mut() {
        replicate
            .replication_clients_cache
            .retain(|_, visibility| match visibility {
                ClientVisibility::Gained => {
                    *visibility = ClientVisibility::Maintained;
                    true
                }
                ClientVisibility::Lost => false,
                ClientVisibility::Maintained => true,
            });
    }
}

/// Clear out the room metadata for any entity that was ever replicated
fn clean_entity_despawns<P: Protocol>(
    mut server: ResMut<Server<P>>,
    mut despawned: RemovedComponents<DespawnTracker>,
) {
    for entity in despawned.read() {
        server.room_manager.entity_despawn(entity);
    }
}
