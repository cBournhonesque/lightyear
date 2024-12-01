/*! Room-based network relevance module, where you can use semi-static rooms to manage network relevance

# Room

Rooms are used to provide interest management in a semi-static way.
Entities and Clients can be added to multiple rooms.

If an entity and a client are in the same room, then the entity will be relevant to the client.
If an entity leaves a room that a client is in, or if a client leaves a room that an entity is in,
then the entity won't be relevant to that client (and will despawned for that client)

You can also find more information in the [book](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/interest_management.html).

## Example

This can be useful for games where you have physical instances of rooms:
- a RPG where you can have different rooms (tavern, cave, city, etc.)
- a server could have multiple lobbies, and each lobby is in its own room
- a map could be divided into a grid of 2D squares, where each square is its own room

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

fn room_system(mut manager: ResMut<RoomManager>) {
   // the entity will now be visible to the client
   manager.add_client(ClientId::Netcode(0), RoomId(0));
   manager.add_entity(Entity::PLACEHOLDER, RoomId(0));
}
```

## Implementation

Under the hood, the [`RoomManager`] uses the same functions as in the immediate-mode [`RelevanceManager`],
it just caches the room metadata to keep track of the relevance of entities.

*/

use bevy::app::App;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bevy::utils::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tracing::trace;

use crate::connection::id::ClientId;
use crate::prelude::server::is_started;

use crate::server::relevance::immediate::{NetworkRelevanceSet, RelevanceManager};
use crate::shared::sets::{InternalReplicationSet, ServerMarker};

use bevy::utils::hashbrown;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;
type EntityHashSet<K> = hashbrown::HashSet<K, EntityHash>;

/// Id for a [`Room`], which is used to perform interest management.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, Hash, PartialEq, Default, Reflect)]
pub struct RoomId(pub u64);

impl From<Entity> for RoomId {
    fn from(value: Entity) -> Self {
        RoomId(value.to_bits())
    }
}

impl From<ClientId> for RoomId {
    fn from(value: ClientId) -> Self {
        RoomId(value.to_bits())
    }
}

/// Resource that will track any changes in the rooms
/// (we cannot use bevy `Events` directly because we don't need to send this every frame.
/// Also, we only need to keep track of updates for each send_interval frame. That means that if an entity
/// leaves then re-joins a room within the same send_interval period, we don't need to send any update)
///
/// This will be cleared every time the Server sends updates to the Client (every send_interval)
#[derive(Resource, Debug, Default)]
struct RoomEvents {
    client_enter_room: HashMap<ClientId, HashSet<RoomId>>,
    client_leave_room: HashMap<ClientId, HashSet<RoomId>>,
    entity_enter_room: EntityHashMap<Entity, HashSet<RoomId>>,
    entity_leave_room: EntityHashMap<Entity, HashSet<RoomId>>,
}

#[derive(Resource, Debug, Default)]
struct VisibilityEvents {
    gained: HashMap<ClientId, Entity>,
    lost: HashMap<ClientId, Entity>,
}

#[derive(Default, Debug)]
struct RoomData {
    /// List of rooms that a client is in
    client_to_rooms: HashMap<ClientId, HashSet<RoomId>>,
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
    pub clients: HashSet<ClientId>,
    /// list of entities that are in the room
    pub entities: EntityHashSet<Entity>,
}

/// Manager responsible for handling rooms
#[derive(Default, Resource)]
pub struct RoomManager {
    events: RoomEvents,
    data: RoomData,
}

/// Plugin used to handle interest managements via [`Room`]s
#[derive(Default)]
pub struct RoomPlugin;

/// System sets related to Rooms
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomSystemSets {
    /// Use all the room events that happened, and use those to update
    /// the replication caches
    UpdateReplicationCaches,
}

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<RoomManager>();
        // SETS
        app.configure_sets(
            PostUpdate,
            (
                (
                    // the room events must be processed before the relevance events
                    RoomSystemSets::UpdateReplicationCaches,
                    NetworkRelevanceSet::UpdateRelevance,
                )
                    .run_if(is_started)
                    .chain(),
                // the room systems can run every send_interval
                RoomSystemSets::UpdateReplicationCaches
                    .in_set(InternalReplicationSet::<ServerMarker>::SendMessages), // .run_if(is_server_ready_to_send),
            ),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            (
                systems::buffer_room_relevance_events
                    .in_set(RoomSystemSets::UpdateReplicationCaches),
            ),
        );
        app.add_observer(systems::handle_client_disconnect);
        app.add_observer(systems::clean_entity_despawns);
    }
}

impl RoomManager {
    /// Remove the client from all the rooms it was in
    fn client_disconnect(&mut self, client_id: ClientId) {
        if let Some(rooms) = self.data.client_to_rooms.remove(&client_id) {
            for room_id in rooms {
                self.remove_client_internal(room_id, client_id);
            }
        }
    }

    /// Remove the entity from all the rooms it was in
    fn entity_despawn(&mut self, entity: Entity) {
        if let Some(rooms) = self.data.entity_to_rooms.remove(&entity) {
            for room_id in rooms {
                self.remove_entity_internal(room_id, entity);
            }
        }
    }

    /// Add a client to the [`Room`]
    pub fn add_client(&mut self, client_id: ClientId, room_id: RoomId) {
        self.add_client_internal(room_id, client_id)
    }

    /// Remove a client from the [`Room`]
    pub fn remove_client(&mut self, client_id: ClientId, room_id: RoomId) {
        self.remove_client_internal(room_id, client_id)
    }

    /// Add an entity to the [`Room`]
    pub fn add_entity(&mut self, entity: Entity, room_id: RoomId) {
        self.add_entity_internal(room_id, entity)
    }

    /// Remove an entity from the [`Room`]
    pub fn remove_entity(&mut self, entity: Entity, room_id: RoomId) {
        self.remove_entity_internal(room_id, entity)
    }

    /// Returns true if the [`Room`] contains the [`ClientId`]
    pub fn has_client_id(&self, client_id: ClientId, room_id: RoomId) -> bool {
        self.has_client_internal(room_id, client_id)
    }

    /// Returns true if the [`Room`] contains the [`Entity`]
    pub fn has_entity(&self, entity: Entity, room_id: RoomId) -> bool {
        self.has_entity_internal(room_id, entity)
    }

    /// Get a room by its [`RoomId`]
    pub fn get_room(&self, room_id: RoomId) -> Option<&Room> {
        self.data.rooms.get(&room_id)
    }

    /// Get a room by its [`RoomId`]
    ///
    /// Panics if the room does not exist.
    pub fn room(&self, room_id: RoomId) -> &Room {
        self.data.rooms.get(&room_id).unwrap()
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

    fn has_entity_internal(&self, room_id: RoomId, entity: Entity) -> bool {
        self.data
            .rooms
            .get(&room_id)
            .map_or_else(|| false, |room| room.entities.contains(&entity))
    }

    /// Returns true if the room contains the client
    fn has_client_internal(&self, room_id: RoomId, client_id: ClientId) -> bool {
        self.data
            .rooms
            .get(&room_id)
            .map_or_else(|| false, |room| room.clients.contains(&client_id))
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

pub(super) mod systems {
    use super::*;
    use crate::prelude::ReplicationGroup;
    use crate::server::events::DisconnectEvent;
    use bevy::prelude::Trigger;

    /// Clear the internal room buffers when a client disconnects
    pub fn handle_client_disconnect(
        trigger: Trigger<DisconnectEvent>,
        mut room_manager: ResMut<RoomManager>,
    ) {
        room_manager.client_disconnect(trigger.event().client_id);
    }

    // TODO: (perf) split this into 4 separate functions that access RoomManager in parallel?
    //  (we only use the ids in events, so we can read them in parallel)
    /// Update each entities' replication-client-list based on the room events
    /// Note that the rooms' entities/clients have already been updated at this point
    pub fn buffer_room_relevance_events(
        mut room_manager: ResMut<RoomManager>,
        mut relevance_manager: ResMut<RelevanceManager>,
    ) {
        if !room_manager.events.is_empty() {
            trace!(?room_manager.events, "Room events");
        }
        // enable split borrows by reborrowing Mut
        let room_manager = &mut *room_manager;

        // NOTE: we handle leave room events before join room events so that if an entity leaves room 1 to join room 2
        //  and the client is in both rooms, the entity does not get despawned

        // entity left room
        for (entity, rooms) in room_manager.events.entity_leave_room.drain() {
            // for each room left, update the entity's client relevance list if the client was in the room
            rooms.into_iter().for_each(|room_id| {
                let room = room_manager.data.rooms.get(&room_id).unwrap();
                room.clients.iter().for_each(|client_id| {
                    trace!("entity {entity:?} left room {room:?}. Sending lost relevance to client {client_id:?}");
                    relevance_manager.lose_relevance(*client_id, entity);
                });
            });
        }
        // entity joined room
        for (entity, rooms) in room_manager.events.entity_enter_room.drain() {
            // for each room joined, update the entity's client relevance list
            rooms.into_iter().for_each(|room_id| {
                let room = room_manager.data.rooms.get(&room_id).unwrap();
                room.clients.iter().for_each(|client_id| {
                    trace!("entity {entity:?} joined room {room:?}. Sending gained relevance to client {client_id:?}");
                    relevance_manager.gain_relevance(*client_id, entity);
                });
            });
        }
        // client left room: update all the entities that are in that room
        for (client_id, rooms) in room_manager.events.client_leave_room.drain() {
            rooms.into_iter().for_each(|room_id| {
                let room = room_manager.data.rooms.get(&room_id).unwrap();
                room.entities.iter().for_each(|entity| {
                    trace!("client {client_id:?} left room {room:?}. Sending lost relevance to entity {entity:?}");
                    relevance_manager.lose_relevance(client_id, *entity);
                });
            });
        }
        // client joined room: update all the entities that are in that room
        for (client_id, rooms) in room_manager.events.client_enter_room.drain() {
            rooms.into_iter().for_each(|room_id| {
                let room = room_manager.data.rooms.get(&room_id).unwrap();
                room.entities.iter().for_each(|entity| {
                    trace!("client {client_id:?} joined room {room:?}. Sending gained relevance to entity {entity:?}");
                    relevance_manager.gain_relevance(client_id, *entity);
                });
            });
        }
    }

    /// Clear out the room metadata for any entity that was ever replicated
    pub fn clean_entity_despawns(
        // we use the removal of ReplicationGroup to detect if the entity was despawned
        trigger: Trigger<OnRemove, ReplicationGroup>,
        mut room_manager: ResMut<RoomManager>,
    ) {
        room_manager.entity_despawn(trigger.entity());
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::Events;
    use bevy::utils::HashMap;

    use crate::prelude::client::*;
    use crate::prelude::server::Replicate;
    use crate::prelude::*;
    use crate::server::relevance::immediate::systems::{
        add_cached_network_relevance, update_relevance_from_events,
    };
    use crate::server::relevance::immediate::{CachedNetworkRelevance, ClientRelevance};
    use crate::shared::replication::components::NetworkRelevanceMode;
    use crate::tests::stepper::BevyStepper;

    use super::systems::buffer_room_relevance_events;

    use super::*;

    #[test]
    // client is in a room
    // we add an entity to that room, then we remove it
    fn test_add_remove_entity_room() {
        let mut stepper = BevyStepper::default();

        // Client joins room
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);

        // Spawn an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();

        stepper.frame_step();
        stepper.frame_step();

        // Check room states
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .has_client_id(client_id, room_id));

        // Add the entity in the same room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .events
            .entity_enter_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CachedNetworkRelevance>()
            .is_some());
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);

        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Gained)])
        );

        stepper.frame_step();
        // Bookkeeping should get applied
        // Check room states
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );

        // Check that the entity gets replicated to client
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Events<EntitySpawnEvent>>()
                .len(),
            1
        );
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();

        // Remove the entity from the room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(server_entity, room_id);
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .events
            .entity_leave_room
            .get(&server_entity)
            .unwrap()
            .contains(&room_id));
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Lost)])
        );
        stepper.frame_step();
        // after bookkeeping, the entity should not have any clients in its replication cache
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CachedNetworkRelevance>()
            .unwrap()
            .clients_cache
            .is_empty());

        stepper.frame_step();
        // Check that the entity gets despawned on client
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Events<EntityDespawnEvent>>()
                .len(),
            1
        );
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_entity)
            .is_err());
    }

    #[test]
    // entity is in a room
    // we add a client to that room, then we remove it
    fn test_add_remove_client_room() {
        let mut stepper = BevyStepper::default();

        // Client joins room
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);

        // Spawn an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);

        stepper.frame_step();
        stepper.frame_step();

        // Check room states
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));

        // Add the client in the same room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .events
            .client_enter_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Gained)])
        );

        stepper.frame_step();
        // Bookkeeping should get applied
        // Check room states
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .has_entity(server_entity, room_id));
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );

        // Check that the entity gets replicated to client
        stepper.frame_step();
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Events<EntitySpawnEvent>>()
                .len(),
            1
        );
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();

        // Remove the client from the room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        assert!(stepper
            .server_app
            .world()
            .resource::<RoomManager>()
            .events
            .client_leave_room
            .get(&client_id)
            .unwrap()
            .contains(&room_id));
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Lost)])
        );
        stepper.frame_step();
        // after bookkeeping, the entity should not have any clients in its replication cache
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CachedNetworkRelevance>()
            .unwrap()
            .clients_cache
            .is_empty());

        stepper.frame_step();
        // Check that the entity gets despawned on client
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<Events<EntityDespawnEvent>>()
                .len(),
            1
        );
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_entity)
            .is_err());
    }

    /// The client is in a room with the entity
    /// We move the client and the entity to a different room (client first, then entity)
    /// There should be no change in relevance
    #[test]
    fn test_move_client_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client join room
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);

        // Spawn an entity on server, in the same room
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );

        let new_room_id = RoomId(1);
        // client leaves previous room and joins new room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // entity leaves previous room and joins new room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(server_entity, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );
    }

    /// The client is in room A and B
    /// Entity is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);
        let new_room_id = RoomId(1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // Spawn an entity on server, in room 1
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );

        // entity leaves previous room and joins new room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(server_entity, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );
    }

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_client_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);
        let new_room_id = RoomId(1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        // Spawn an entity on server, in room 1
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Gained)])
        );
        // apply bookkeeping
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );

        // client leaves previous room and joins new room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, new_room_id);
        // Run update replication cache once
        stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(server_entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from([(client_id, ClientRelevance::Maintained)])
        );
    }

    // TODO: check that entity despawn/client disconnect cleans the room metadata
}
