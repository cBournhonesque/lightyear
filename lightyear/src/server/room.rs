//! # Room
//!
//! This module contains the room system, which is used to perform interest management. (being able to predict certain entities to certain clients only).
//! You can also find more information in the [book](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/interest_management.html).
use bevy::app::App;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{
    Entity, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PostUpdate, Query, RemovedComponents,
    Res, ResMut, Resource, SystemSet,
};
use bevy::utils::{HashMap, HashSet};
use tracing::info;

use crate::connection::netcode::ClientId;
use crate::prelude::ReplicationSet;
use crate::protocol::Protocol;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use crate::shared::time_manager::is_ready_to_send;
use crate::utils::wrapping_id::wrapping_id;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;
type EntityHashSet<K> = hashbrown::HashSet<K, EntityHash>;

// Id for a [`Room`], which is used to perform interest management.
wrapping_id!(RoomId);

/// Resource that will track any changes in the rooms
/// (we cannot use bevy `Events` directly because we don't need to send this every frame.
/// Also, we only need to keep track of updates for each send_interval frame. That means that if an entity
/// leaves then re-joins a room within the same send_interval period, we don't need to send any update)
///
/// This will be cleared every time the Server sends updates to the Client (every send_interval)
#[derive(Resource, Debug, Default)]
struct RoomEvents {
    client_enter_room: EntityHashMap<ClientId, HashSet<RoomId>>,
    client_leave_room: EntityHashMap<ClientId, HashSet<RoomId>>,
    entity_enter_room: EntityHashMap<Entity, HashSet<RoomId>>,
    entity_leave_room: EntityHashMap<Entity, HashSet<RoomId>>,
}

#[derive(Default, Debug)]
struct RoomData {
    /// List of rooms that a client is in
    client_to_rooms: EntityHashMap<ClientId, HashSet<RoomId>>,
    /// List of rooms that an entity is in
    entity_to_rooms: EntityHashMap<Entity, HashSet<RoomId>>,
    /// Mapping from [`RoomId`] to the [`Room`]
    rooms: HashMap<RoomId, Room>,
}

/// A [`Room`] is a data structure that is used to perform interest management.
///
/// It holds a list of clients and entities that are in the room.
/// An entity is visible to a client only if it is in the same room as the client.
///
/// Entities and clients can belong to multiple rooms, they just need to both be present in one room
/// for the entity to be replicated to the client.
#[derive(Debug, Default)]
pub struct Room {
    /// list of clients that are in the room
    clients: EntityHashSet<ClientId>,
    /// list of entities that are in the room
    entities: EntityHashSet<Entity>,
}

/// Manager responsible for handling rooms
#[derive(Default, Resource)]
pub struct RoomManager {
    events: RoomEvents,
    data: RoomData,
}

impl RoomManager {
    /// Returns a mutable reference to the room with the given id
    pub fn room_mut(&mut self, id: RoomId) -> RoomMut {
        RoomMut { id, manager: self }
    }

    /// Returns a reference to the room with the given id
    pub fn room(&self, id: RoomId) -> RoomRef {
        RoomRef { id, manager: self }
    }
}

/// Plugin used to handle interest managements via [`Room`]s
pub struct RoomPlugin<P: Protocol> {
    _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for RoomPlugin<P> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
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
        // RESOURCES
        app.init_resource::<RoomManager>();
        // SETS
        app.configure_sets(
            PostUpdate,
            (
                (
                    // update replication caches must happen before replication
                    RoomSystemSets::UpdateReplicationCaches,
                    ReplicationSet::All,
                    RoomSystemSets::RoomBookkeeping,
                )
                    .chain(),
                // the room systems can run every send_interval
                (
                    RoomSystemSets::UpdateReplicationCaches,
                    RoomSystemSets::RoomBookkeeping,
                )
                    .run_if(is_ready_to_send),
            ),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            (
                update_entity_replication_cache::<P>
                    .in_set(RoomSystemSets::UpdateReplicationCaches),
                (clear_entity_replication_cache::<P>, clean_entity_despawns)
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
                self.remove_client_internal(room_id, client_id);
            }
        }
    }

    /// Remove the entity from all the rooms it was in
    pub(crate) fn entity_despawn(&mut self, entity: Entity) {
        if let Some(rooms) = self.data.entity_to_rooms.remove(&entity) {
            for room_id in rooms {
                RoomMut::new(self, room_id).remove_entity(entity);
                self.remove_entity_internal(room_id, entity);
            }
        }
    }
    /// Add a client to the [`Room`]
    pub fn add_client(&mut self, client_id: ClientId, room_id: RoomId) {
        self.room_mut(room_id).add_client(client_id)
    }

    /// Remove a client from the [`Room`]
    pub fn remove_client(&mut self, client_id: ClientId, room_id: RoomId) {
        self.room_mut(room_id).remove_client(client_id)
    }

    /// Add an entity to the [`Room`]
    pub fn add_entity(&mut self, entity: Entity, room_id: RoomId) {
        self.room_mut(room_id).add_entity(entity)
    }

    /// Remove an entity from the [`Room`]
    pub fn remove_entity(&mut self, entity: Entity, room_id: RoomId) {
        self.room_mut(room_id).remove_entity(entity)
    }

    /// Returns true if the [`Room`] contains the [`ClientId`]
    pub fn has_client_id(&self, client_id: ClientId, room_id: RoomId) -> bool {
        self.room(room_id).has_client_id(client_id)
    }

    /// Returns true if the [`Room`] contains the [`Entity`]
    pub fn has_entity(&self, entity: Entity, room_id: RoomId) -> bool {
        self.room(room_id).has_entity(entity)
    }

    /// Get a room by its [`RoomId`]
    pub fn get_room(&self, room_id: RoomId) -> Option<&Room> {
        self.data.rooms.get(&room_id)
    }

    fn add_client_internal(&mut self, room_id: RoomId, client_id: ClientId) {
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

    fn remove_client_internal(&mut self, room_id: RoomId, client_id: ClientId) {
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

    fn add_entity_internal(&mut self, room_id: RoomId, entity: Entity) {
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

    fn remove_entity_internal(&mut self, room_id: RoomId, entity: Entity) {
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

    /// Add a client to the room
    pub fn add_client(&mut self, client_id: ClientId) {
        self.manager.add_client_internal(self.id, client_id)
    }

    /// Remove a client from the room
    pub fn remove_client(&mut self, client_id: ClientId) {
        self.manager.remove_client_internal(self.id, client_id)
    }

    /// Add an entity to the room
    pub fn add_entity(&mut self, entity: Entity) {
        self.manager.add_entity_internal(self.id, entity)
    }

    /// Remove an entity from the room
    pub fn remove_entity(&mut self, entity: Entity) {
        self.manager.remove_entity_internal(self.id, entity)
    }

    /// Returns true if the room contains the client
    pub fn has_client_id(&self, client_id: ClientId) -> bool {
        self.manager
            .get_room(self.id)
            .map_or_else(|| false, |room| room.clients.contains(&client_id))
    }

    /// Returns true if the room contains the entity
    pub fn has_entity(&mut self, entity: Entity) -> bool {
        self.manager
            .get_room(self.id)
            .map_or_else(|| false, |room| room.entities.contains(&entity))
    }
}

/// Convenient wrapper to inspect a room
pub struct RoomRef<'s> {
    pub(crate) id: RoomId,
    pub(crate) manager: &'s RoomManager,
}

impl<'s> RoomRef<'s> {
    fn new(manager: &'s RoomManager, id: RoomId) -> Self {
        Self { id, manager }
    }

    pub fn has_client_id(&self, client_id: ClientId) -> bool {
        self.manager
            .get_room(self.id)
            .map_or_else(|| false, |room| room.clients.contains(&client_id))
    }

    pub fn has_entity(&mut self, entity: Entity) -> bool {
        self.manager
            .get_room(self.id)
            .map_or_else(|| false, |room| room.entities.contains(&entity))
    }
}

impl RoomEvents {
    fn is_empty(&self) -> bool {
        self.client_enter_room.is_empty()
            && self.client_leave_room.is_empty()
            && self.entity_enter_room.is_empty()
            && self.entity_leave_room.is_empty()
    }
    fn clear(&mut self) {
        self.client_enter_room.clear();
        self.client_leave_room.clear();
        self.entity_enter_room.clear();
        self.entity_leave_room.clear();
    }

    /// A client joined a room
    pub fn client_enter_room(&mut self, room_id: RoomId, client_id: ClientId) {
        // if the client had left the room and re-entered, no need to track the enter
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
        // if the client had entered the room and left, no need to track the leaving
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

// TODO: this should not be public?
/// Event related to [`Entities`](Entity) which are visible to a client
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ClientVisibility {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
}

// TODO: (perf) split this into 4 separate functions that access RoomManager in parallel?
//  (we only use the ids in events, so we can read them in parallel)
/// Update each entities' replication-client-list based on the room events
/// Note that the rooms' entities/clients have already been updated at this point
fn update_entity_replication_cache<P: Protocol>(
    mut room_manager: ResMut<RoomManager>,
    mut query: Query<&mut Replicate<P>>,
) {
    if !room_manager.events.is_empty() {
        info!(?room_manager.events, "Room events");
    }
    // enable split borrows by reborrowing Mut
    let room_manager = &mut *room_manager;

    // NOTE: we handle leave room events before join room events so that if an entity leaves room 1 to join room 2
    //  and the client is in both rooms, the entity does not get despawned

    // entity left room
    for (entity, rooms) in room_manager.events.entity_leave_room.drain() {
        // for each room left, update the entity's client visibility list if the client was in the room
        rooms.into_iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(&room_id).unwrap();
            room.clients.iter().for_each(|client_id| {
                if let Ok(mut replicate) = query.get_mut(entity) {
                    if let Some(visibility) = replicate.replication_clients_cache.get_mut(client_id)
                    {
                        *visibility = ClientVisibility::Lost;
                    }
                }
            });
        });
    }
    // entity joined room
    for (entity, rooms) in room_manager.events.entity_enter_room.drain() {
        // for each room joined, update the entity's client visibility list
        rooms.into_iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(&room_id).unwrap();
            room.clients.iter().for_each(|client_id| {
                if let Ok(mut replicate) = query.get_mut(entity) {
                    replicate
                        .replication_clients_cache
                        .entry(*client_id)
                        .and_modify(|vis| {
                            // if the visibility was lost above, then that means that the entity was visible
                            // for this client, so we just maintain it instead
                            if *vis == ClientVisibility::Lost {
                                *vis = ClientVisibility::Maintained
                            }
                        })
                        // if the entity was not visible, the visibility is gained
                        .or_insert(ClientVisibility::Gained);
                }
            });
        });
    }
    // client left room: update all the entities that are in that room
    for (client_id, rooms) in room_manager.events.client_leave_room.drain() {
        rooms.into_iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(&room_id).unwrap();
            room.entities.iter().for_each(|entity| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    if let Some(visibility) =
                        replicate.replication_clients_cache.get_mut(&client_id)
                    {
                        *visibility = ClientVisibility::Lost;
                    }
                }
            });
        });
    }
    // client joined room: update all the entities that are in that room
    for (client_id, rooms) in room_manager.events.client_enter_room.drain() {
        rooms.into_iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(&room_id).unwrap();
            room.entities.iter().for_each(|entity| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    replicate
                        .replication_clients_cache
                        .entry(client_id)
                        .and_modify(|vis| {
                            // if the visibility was lost above, then that means that the entity was visible
                            // for this client, so we just maintain it instead
                            if *vis == ClientVisibility::Lost {
                                *vis = ClientVisibility::Maintained
                            }
                        })
                        // if the entity was not visible, the visibility is gained
                        .or_insert(ClientVisibility::Gained);
                }
            });
        });
    }
}

/// After replication, update the Replication Cache:
/// - Visibility Gained becomes Visibility Maintained
/// - Visibility Lost gets removed from the cache
fn clear_entity_replication_cache<P: Protocol>(mut query: Query<&mut Replicate<P>>) {
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
fn clean_entity_despawns(
    mut room_manager: ResMut<RoomManager>,
    mut despawned: RemovedComponents<DespawnTracker>,
) {
    for entity in despawned.read() {
        room_manager.entity_despawn(entity);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::Events;
    use bevy::utils::{Duration, HashMap};

    use crate::prelude::client::*;
    use crate::prelude::*;
    use crate::shared::replication::components::ReplicationMode;
    use crate::tests::protocol::Replicate;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    use super::*;

    #[test]
    // client is in a room
    // we add an entity to that room, then we remove it
    fn test_add_remove_entity_room() {
        let mut stepper = BevyStepper::default();

        // Client joins room
        let client_id = 111;
        let room_id = RoomId(0);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_client(client_id);

        // Spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                replication_mode: ReplicationMode::Room,
                ..Default::default()
            })
            .id();

        stepper.frame_step();
        stepper.frame_step();

        // Check room states
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .has_client_id(client_id, room_id));

        // Add the entity in the same room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_entity(server_entity);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .entity_enter_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Gained)])
        );

        stepper.frame_step();
        // Bookkeeping should get applied
        // Check room states
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );

        // Check that the entity gets replicated to client
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world
                .resource::<Events<EntitySpawnEvent>>()
                .len(),
            1
        );
        let client_entity = *stepper
            .client_app
            .world
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();

        // Remove the entity from the room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .remove_entity(server_entity);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .entity_leave_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Lost)])
        );
        stepper.frame_step();
        // after bookkeeping, the entity should not have any clients in its replication cache
        assert!(stepper
            .server_app
            .world
            .entity(server_entity)
            .get::<Replicate>()
            .unwrap()
            .replication_clients_cache
            .is_empty());

        stepper.frame_step();
        // Check that the entity gets despawned on client
        assert_eq!(
            stepper
                .client_app
                .world
                .resource::<Events<EntityDespawnEvent>>()
                .len(),
            1
        );
        assert!(stepper.client_app.world.get_entity(client_entity).is_none());
    }

    #[test]
    // entity is in a room
    // we add a client to that room, then we remove it
    fn test_add_remove_client_room() {
        let mut stepper = BevyStepper::default();

        // Client joins room
        let client_id = 111;
        let room_id = RoomId(0);

        // Spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                replication_mode: ReplicationMode::Room,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_entity(server_entity);

        stepper.frame_step();
        stepper.frame_step();

        // Check room states
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));

        // Add the client in the same room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_client(client_id);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .client_enter_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Gained)])
        );

        stepper.frame_step();
        // Bookkeeping should get applied
        // Check room states
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );

        // Check that the entity gets replicated to client
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world
                .resource::<Events<EntitySpawnEvent>>()
                .len(),
            1
        );
        let client_entity = *stepper
            .client_app
            .world
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();

        // Remove the client from the room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .remove_client(client_id);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .client_leave_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Lost)])
        );
        stepper.frame_step();
        // after bookkeeping, the entity should not have any clients in its replication cache
        assert!(stepper
            .server_app
            .world
            .entity(server_entity)
            .get::<Replicate>()
            .unwrap()
            .replication_clients_cache
            .is_empty());

        stepper.frame_step();
        // Check that the entity gets despawned on client
        assert_eq!(
            stepper
                .client_app
                .world
                .resource::<Events<EntityDespawnEvent>>()
                .len(),
            1
        );
        assert!(stepper.client_app.world.get_entity(client_entity).is_none());
    }

    /// The client is in a room with the entity
    /// We move the client and the entity to a different room (client first, then entity)
    /// There should be no change in visibility
    #[test]
    fn test_move_client_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client join room
        let client_id = 111;
        let room_id = RoomId(0);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_client(client_id);
        // Spawn an entity on server, in the same room
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                replication_mode: ReplicationMode::Room,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_entity(server_entity);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );

        let new_room_id = RoomId(1);
        // client leaves previous room and joins new room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // entity leaves previous room and joins new room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .remove_entity(server_entity, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );
    }

    /// The client is in room A and B
    /// Entity is in room A and moves to room B
    /// There should be no change in visibility
    #[test]
    fn test_move_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = 111;
        let room_id = RoomId(0);
        let new_room_id = RoomId(1);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // Spawn an entity on server, in room 1
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                replication_mode: ReplicationMode::Room,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );

        // entity leaves previous room and joins new room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .remove_entity(server_entity, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );
    }

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in visibility
    #[test]
    fn test_move_client_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = 111;
        let room_id = RoomId(0);
        let new_room_id = RoomId(1);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        // Spawn an entity on server, in room 1
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                replication_mode: ReplicationMode::Room,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );

        // client leaves previous room and joins new room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<Replicate>()
                .unwrap()
                .replication_clients_cache,
            HashMap::from([(client_id, ClientVisibility::Maintained)])
        );
    }

    // TODO: check that entity despawn/client disconnect cleans the room metadata
}
