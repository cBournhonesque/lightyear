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
use bevy::ecs::entity::hash_map::EntityHashMap;
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
use bevy::ecs::entity::{hash_map::EntityHashMap, hash_set::EntityHashSet};
use bevy::platform::collections::{hash_map::Entry, HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::Reflect;

use serde::{Deserialize, Serialize};

use crate::connection::id::ClientId;
use crate::prelude::server::is_started;

use crate::server::relevance::immediate::{NetworkRelevanceSet, RelevanceEvents, RelevanceManager};
use crate::shared::sets::{InternalReplicationSet, ServerMarker};

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

#[derive(Resource, Debug, Default)]
struct VisibilityEvents {
    gained: HashMap<ClientId, Entity>,
    lost: HashMap<ClientId, Entity>,
}

#[derive(Default, Debug, Reflect)]
struct RoomData {
    /// List of rooms that a client is in
    client_to_rooms: HashMap<ClientId, HashSet<RoomId>>,
    /// List of rooms that an entity is in
    entity_to_rooms: EntityHashMap<HashSet<RoomId>>,
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
#[derive(Debug, Default, Reflect)]
pub struct Room {
    /// list of clients that are in the room
    pub clients: HashSet<ClientId>,
    /// list of entities that are in the room
    pub entities: EntityHashSet,
}

impl Room {
    fn is_empty(&self) -> bool {
        self.clients.is_empty() && self.entities.is_empty()
    }
}

/// Manager responsible for handling rooms
#[derive(Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct RoomManager {
    events: RelevanceEvents,
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
        // REFLECT
        app.register_type::<(RoomManager, RoomData)>();
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

    /// Remove all clients from a room
    pub fn remove_clients(&mut self, room_id: RoomId) {
        let clients = self
            .data
            .rooms
            .get(&room_id)
            .map_or(vec![], |r| r.clients.iter().copied().collect());
        clients.iter().for_each(|c| {
            self.remove_client(*c, room_id);
        });
    }

    /// Remove all entities from a room
    pub fn remove_entities(&mut self, room_id: RoomId) {
        let entities = self
            .data
            .rooms
            .get(&room_id)
            .map_or(vec![], |r| r.entities.iter().copied().collect());
        entities.iter().for_each(|e| {
            self.remove_entity(*e, room_id);
        });
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
        let room = self.data.rooms.entry(room_id).or_default();
        room.clients.insert(client_id);
        room.entities.iter().for_each(|e| {
            self.events.gain_relevance_internal(client_id, *e);
        });
    }

    fn remove_client_internal(&mut self, room_id: RoomId, client_id: ClientId) {
        self.data
            .client_to_rooms
            .entry(client_id)
            .or_default()
            .remove(&room_id);
        if let Entry::Occupied(mut o) = self.data.rooms.entry(room_id) {
            o.get_mut().clients.remove(&client_id);
            o.get().entities.iter().for_each(|e| {
                self.events.lose_relevance_internal(client_id, *e);
            });
            if o.get().is_empty() {
                o.remove();
            }
        }
    }

    fn add_entity_internal(&mut self, room_id: RoomId, entity: Entity) {
        self.data
            .entity_to_rooms
            .entry(entity)
            .or_default()
            .insert(room_id);
        let room = self.data.rooms.entry(room_id).or_default();
        room.entities.insert(entity);
        room.clients.iter().for_each(|c| {
            self.events.gain_relevance_internal(*c, entity);
        });
    }

    fn remove_entity_internal(&mut self, room_id: RoomId, entity: Entity) {
        self.data
            .entity_to_rooms
            .entry(entity)
            .or_default()
            .remove(&room_id);
        if let Entry::Occupied(mut o) = self.data.rooms.entry(room_id) {
            o.get_mut().entities.remove(&entity);
            o.get().clients.iter().for_each(|c| {
                self.events.lose_relevance_internal(*c, entity);
            });
            if o.get().is_empty() {
                o.remove();
            }
        }
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
        relevance_manager.events.update(&mut room_manager.events);
    }

    /// Clear out the room metadata for any entity that was ever replicated
    pub fn clean_entity_despawns(
        // we use the removal of ReplicationGroup to detect if the entity was despawned
        trigger: Trigger<OnRemove, ReplicationGroup>,
        mut room_manager: ResMut<RoomManager>,
    ) {
        room_manager.entity_despawn(trigger.target());
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::Events;

    use crate::prelude::client::*;
    use crate::prelude::server::Replicate;
    use crate::prelude::*;
    use crate::server::relevance::immediate::systems::{
        add_cached_network_relevance, update_relevance_from_events,
    };
    use crate::server::relevance::immediate::{CachedNetworkRelevance, ClientRelevance};
    use crate::shared::replication::components::NetworkRelevanceMode;
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
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
            .gained
            .get(&client_id)
            .unwrap()
            .contains(&server_entity));
        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CachedNetworkRelevance>()
            .is_some());
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
            .lost
            .get(&client_id)
            .unwrap()
            .contains(&server_entity));
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Lost)])
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
            .gained
            .get(&client_id)
            .unwrap()
            .contains(&server_entity));
        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
            .lost
            .get(&client_id)
            .unwrap()
            .contains(&server_entity));
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Lost)])
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(server_entity, room_id);
        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
        let _ = stepper
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
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
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
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
            HashMap::from_iter([(client_id, ClientRelevance::Maintained)])
        );
    }

    /// The entity and client are in room A
    /// Entity,client leave room at the same time
    ///
    /// Entity-Client should lose relevance (not in the same room anymore)
    #[test]
    fn test_client_entity_both_leave_room() {
        let mut stepper = BevyStepper::default();
        let client_id = ClientId::Netcode(111);
        let room_id = RoomId(0);

        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(client_id, room_id);
        // Spawn an entity on server, in room 1
        let entity = stepper
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
            .add_entity(entity, room_id);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);

        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from_iter([(client_id, ClientRelevance::Gained)])
        );

        // Client and entity leave room
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(client_id, room_id);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(entity, room_id);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        // make sure that visibility is lost
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from_iter([(client_id, ClientRelevance::Lost)])
        );
    }
    // TODO: check that entity despawn/client disconnect cleans the room metadata

    // Two clients in same room 1
    // C1 and E1 leaves room 1 and joins room 2: visibility lost (and entity despawned)
    // C1 and E2 leaves room 2 and joins room 1: visibility gained (and entity spawned)
    #[test]
    fn test_multiple_clients_leave_enter_room() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = MultiBevyStepper::default();
        let c1 = ClientId::Netcode(TEST_CLIENT_ID_1);
        let c2 = ClientId::Netcode(TEST_CLIENT_ID_2);
        let r1 = RoomId(1);
        let r2 = RoomId(2);

        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(c1, r1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(c2, r1);
        // spawn one entity for each client
        let entity_1 = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        let entity_2 = stepper
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
            .add_entity(entity_1, r1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(entity_2, r1);

        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(add_cached_network_relevance);

        // Run update replication cache once
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_1)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from_iter([(c1, ClientRelevance::Gained), (c2, ClientRelevance::Gained)])
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache,
            HashMap::from_iter([(c1, ClientRelevance::Gained), (c2, ClientRelevance::Gained)])
        );
        stepper.frame_step();
        stepper.frame_step();
        let c1_entity_1 = stepper
            .client_app_1
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_1)
            .expect("entity 1 was not replicated to client 1");
        let c1_entity_2 = stepper
            .client_app_1
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_2)
            .expect("entity 2 was not replicated to client 1");
        let c2_entity_1 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_1)
            .expect("entity 1 was not replicated to client 2");
        let c2_entity_2 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_2)
            .expect("entity 2 was not replicated to client 2");

        // C1 and E1 leaves room 1 and joins room 2
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(c1, r1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(entity_1, r1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(c1, r2);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(entity_1, r2);

        // check interest management internals
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&c1),
            Some(&ClientRelevance::Lost)
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_1)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&c2),
            Some(&ClientRelevance::Lost)
        );

        // check that the changes were impacted via replication
        // entity_1 should be despawned on c2
        // entity_2 should be despawned on c1
        stepper.frame_step();
        stepper.frame_step();
        assert!(stepper
            .client_app_1
            .world()
            .get_entity(c1_entity_2)
            .is_err());
        assert!(stepper
            .client_app_2
            .world()
            .get_entity(c2_entity_1)
            .is_err());

        // C1 and E1 leaves room 2 and joins room 1
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_client(c1, r2);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .remove_entity(entity_1, r2);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_client(c1, r1);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RoomManager>()
            .add_entity(entity_1, r1);

        // check interest management internals
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(buffer_room_relevance_events);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system_once(update_relevance_from_events);
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&c1),
            Some(&ClientRelevance::Gained)
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .entity(entity_1)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&c2),
            Some(&ClientRelevance::Gained)
        );
        stepper.frame_step();
        stepper.frame_step();
        let c1_entity_2_v2 = stepper
            .client_app_1
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_2)
            .expect("entity 2 was not replicated to client 1");
        assert_ne!(c1_entity_2, c1_entity_2_v2);
        let c2_entity_1_v2 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity_1)
            .expect("entity 1 was not replicated to client 2");
        assert_ne!(c2_entity_1, c2_entity_1_v2);
    }
}
