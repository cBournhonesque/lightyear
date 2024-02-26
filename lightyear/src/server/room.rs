//! # Room
//!
//! This module contains the room system, which is used to perform interest management. (being able to predict certain entities to certain clients only).
//! You can also find more information in the [book](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/interest_management.html).
use bevy::app::App;
use bevy::prelude::{
    Entity, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PostUpdate, Query, RemovedComponents,
    Res, ResMut, Resource, SystemSet,
};
use bevy::utils::{HashMap, HashSet};

use crate::connection::netcode::ClientId;
use crate::prelude::ReplicationSet;
use crate::protocol::Protocol;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use crate::shared::time_manager::is_ready_to_send;
use crate::utils::wrapping_id::wrapping_id;

// Id for a room, used to perform interest management
// An entity will be replicated to a client only if they are in the same room
wrapping_id!(RoomId);

/// Resource that will track any changes in the rooms
/// (we cannot use bevy `Events` directly because we don't need to send this every frame.
/// Also, we only need to keep track of updates for each send_interval frame. That means that if an entity
/// leaves then re-joins a room within the same send_interval period, we don't need to send any update)
///
/// This will be cleared every time the Server sends updates to the Client (every send_interval)
#[derive(Resource, Debug, Default)]
pub struct RoomEvents {
    client_enter_room: HashMap<ClientId, HashSet<RoomId>>,
    client_leave_room: HashMap<ClientId, HashSet<RoomId>>,
    entity_enter_room: HashMap<Entity, HashSet<RoomId>>,
    entity_leave_room: HashMap<Entity, HashSet<RoomId>>,
}

#[derive(Default, Debug)]
pub struct RoomData {
    client_to_rooms: HashMap<ClientId, HashSet<RoomId>>,
    entity_to_rooms: HashMap<Entity, HashSet<RoomId>>,
    rooms: HashMap<RoomId, Room>,
}

#[derive(Debug, Default)]
pub struct Room {
    /// list of clients that are in the room
    clients: HashSet<ClientId>,
    /// list of entities that are in the room
    entities: HashSet<Entity>,
}

#[derive(Default, Resource)]
pub struct RoomManager {
    events: RoomEvents,
    data: RoomData,
}

impl RoomManager {
    // ROOM
    pub fn room_mut(&mut self, id: RoomId) -> RoomMut {
        RoomMut { id, manager: self }
    }

    pub fn room(&self, id: RoomId) -> RoomRef {
        RoomRef { id, manager: self }
    }
}

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
                (
                    clear_entity_replication_cache::<P>,
                    clean_entity_despawns,
                    clear_room_events,
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

/// Convenient wrapper to mutate a room
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
            .data
            .rooms
            .get(&self.id)
            .map_or_else(|| false, |room| room.clients.contains(&client_id))
    }

    pub fn has_entity(&mut self, entity: Entity) -> bool {
        self.manager
            .data
            .rooms
            .get(&self.id)
            .map_or_else(|| false, |room| room.entities.contains(&entity))
    }
}

impl RoomEvents {
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

// TODO: this should not be public
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ClientVisibility {
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
    room_manager: Res<RoomManager>,
    mut query: Query<&mut Replicate<P>>,
) {
    // entity joined room
    for (entity, rooms) in room_manager.events.iter_entity_enter_room() {
        // for each room joined, update the entity's client visibility list
        rooms.iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(room_id).unwrap();
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
    for (entity, rooms) in room_manager.events.iter_entity_leave_room() {
        // for each room left, update the entity's client visibility list if the client was in the room
        rooms.iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(room_id).unwrap();
            room.clients.iter().for_each(|client_id| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    if let Some(visibility) = replicate.replication_clients_cache.get_mut(client_id)
                    {
                        *visibility = ClientVisibility::Lost;
                    }
                }
            });
        });
    }
    // client joined room: update all the entities that are in that room
    for (client_id, rooms) in room_manager.events.iter_client_enter_room() {
        rooms.iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(room_id).unwrap();
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
    for (client_id, rooms) in room_manager.events.iter_client_leave_room() {
        rooms.iter().for_each(|room_id| {
            let room = room_manager.data.rooms.get(room_id).unwrap();
            room.entities.iter().for_each(|entity| {
                if let Ok(mut replicate) = query.get_mut(*entity) {
                    if let Some(visibility) = replicate.replication_clients_cache.get_mut(client_id)
                    {
                        *visibility = ClientVisibility::Lost;
                    }
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

/// Clear every room event that happened
fn clear_room_events(mut room_manager: ResMut<RoomManager>) {
    room_manager.events.clear();
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

    fn setup() -> BevyStepper {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper.init();
        stepper
    }

    #[test]
    // client is in a room
    // we add an entity to that room, then we remove it
    fn test_add_remove_entity_room() {
        let mut stepper = setup();

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
            .data
            .rooms
            .get(&room_id)
            .unwrap()
            .clients
            .contains(&client_id),);

        // Add the entity in the same room
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
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .entity_enter_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
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
            .data
            .rooms
            .get(&room_id)
            .unwrap()
            .entities
            .contains(&server_entity));
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
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .entity_leave_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
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
        let mut stepper = setup();

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
            .data
            .rooms
            .get(&room_id)
            .unwrap()
            .entities
            .contains(&server_entity));

        // Add the client in the same room
        stepper
            .server_app
            .world
            .resource_mut::<RoomManager>()
            .room_mut(room_id)
            .add_client(client_id);
        // Run update replication cache once
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .client_enter_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
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
            .data
            .rooms
            .get(&room_id)
            .unwrap()
            .entities
            .contains(&server_entity));
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
        stepper
            .server_app
            .world
            .run_system_once(update_entity_replication_cache::<MyProtocol>);
        assert!(stepper
            .server_app
            .world
            .resource::<RoomManager>()
            .events
            .client_leave_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
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

    // TODO: check that entity despawn/client disconnect cleans the room metadata

    // TODO: check
}
