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
   manager.add_client(PeerId::Netcode(0), RoomId(0));
   manager.add_entity(Entity::PLACEHOLDER, RoomId(0));
}
```

## Implementation

Under the hood, the [`RoomManager`] uses the same functions as in the immediate-mode [`RelevanceManager`],
it just caches the room metadata to keep track of the relevance of entities.

*/

use bevy::app::App;
use bevy::ecs::entity::{hash_map::EntityHashMap, hash_set::EntityHashSet, EntityIndexMap};
use bevy::platform_support::collections::{hash_map::Entry, HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::Reflect;

use crate::relevance::error::NetworkVisibilityError;
use crate::relevance::immediate::{NetworkRelevancePlugin, NetworkVisibility};
use crate::send::ReplicationBufferSet;
use lightyear_connection::prelude::PeerId;
use serde::{Deserialize, Serialize};


/// A [`Room`] is a data structure that is used to perform interest management.
///
/// It holds a list of clients and entities that are in the room.
/// An entity is visible to a client only if it is in the same room as the client.
///
/// Entities and clients can belong to multiple rooms, they just need to both be present in one room
/// for the entity to be replicated to the client.
#[derive(Debug, Default, Reflect, Component)]
pub struct Room {
    /// list of sender that are in the room
    pub clients: EntityHashSet,
    /// list of entities that are in the room
    pub entities: EntityHashSet,
}


impl Room {
    fn is_empty(&self) -> bool {
        self.clients.is_empty() && self.entities.is_empty()
    }
}


/// Plugin used to handle interest managements via [`Room`]s
#[derive(Default)]
pub struct RoomPlugin;

impl RoomPlugin {
    pub fn handle_room_event(
        trigger: Trigger<RoomEvent>,
        mut room_events: ResMut<RoomEvents>,
        mut query: Query<&mut Room>
    ) -> Result {
        let Ok(mut room) = query.get_mut(trigger.target()) else {
            return NetworkVisibilityError::RoomNotFound(trigger.target()).into()
        };
        match trigger.event() {
            RoomEvent::AddEntity(entity) => {
                room.clients.iter().for_each(|c| {
                    room_events.events.entry(*entity).or_default().gain_visibility(*c)
                });
                room.entities.insert(*entity);
            }
            RoomEvent::RemoveEntity(entity) => {
                 room.clients.iter().for_each(|c| {
                    room_events.events.entry(*entity).or_default().lose_visibility(*c)
                 });
                 room.entities.remove(entity);
            }
            RoomEvent::AddSender(entity) => {
                room.entities.iter().for_each(|e| {
                    room_events.events.entry(*e).or_default().gain_visibility(*entity)
                });
                room.clients.insert(*entity);
            }
            RoomEvent::RemoveSender(entity) => {
                room.entities.iter().for_each(|e| {
                    room_events.events.entry(*e).or_default().lose_visibility(*entity)
                });
                room.clients.insert(*entity);
            }
        }
        Ok(())
    }

    pub fn apply_room_events(
        mut commands: Commands,
        mut room_events: ResMut<RoomEvents>,
        mut query: Query<&mut NetworkVisibility>
    ) {
        // TODO: should we use iter_mut here to keep the allocated NetworkVisibilty?
        room_events.events.drain(..).for_each(|(entity, vis)| {
            if let Ok(mut vis) = query.get_mut(entity) {
                vis.gained.drain().for_each(|sender| {
                    vis.gain_visibility(sender);
                });
                vis.lost.drain().for_each(|sender| {
                    vis.lose_visibility(sender);
                });
            } else {
                commands.entity(entity).insert(vis);
            }
        });
    }
}


/// System sets related to Rooms
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomSet {
    /// Update the [`NetworkVisibility`] components based on room memberships
    ApplyRoomEvents,
}

pub enum RoomEvent {
    AddEntity(Entity),
    RemoveEntity(Entity),
    AddSender(Entity),
    RemoveSender(Entity),
}

#[derive(Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct RoomEvents {
    /// List of events that have been triggered by room events
    ///
    /// We cannot apply the [`RoomEvent`]s directly to the entity's [`NetworVisibility`] because
    /// we need to handle concurrent room moves correctly:
    /// if entity E1 and sender A both leave room R1 and join room R2, the visibility should be
    /// unchanged.
    pub(crate) events: EntityIndexMap<NetworkVisibility>,
}


impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<NetworkRelevancePlugin>() {
            app.add_plugins(NetworkRelevancePlugin);
        }
        // REFLECT
        app.register_type::<(RoomEvents, Room)>();
        // RESOURCES
        app.init_resource::<RoomEvents>();
        // SETS
        app.configure_sets(
            PostUpdate,
            RoomSet::ApplyRoomEvents.in_set(ReplicationBufferSet::BeforeBuffer)
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate, Self::apply_room_events.in_set(RoomSet::ApplyRoomEvents),
        );
        app.add_observer(Self::handle_room_event);
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::collections::HashMap;
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
        let client_id = PeerId::Netcode(111);
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
        let client_id = PeerId::Netcode(111);
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


    // C1 is in room R1 with E1

    // RoomManager.NetworkVisility: E1 visible C1
    // C1 leaves R1: E1 loses C1
    // C1 joins R2:
    // E1 leaves R1:
    // E1 joins R2: E1 gains C1
    // RoomManager.gain_visibility -> nothing changes.

    /// The client is in a room with the entity
    /// We move the client and the entity to a different room (client first, then entity)
    /// There should be no change in relevance
    #[test]
    fn test_move_client_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client join room
        let client_id = PeerId::Netcode(111);
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


    // C1 is in rooms A and B
    // E1 is in room A. RoomManager.NetworkVisibility: E1 visible C1
    // E1 leaves room A: E1 loses C1
    // E1 joins room B: E1 gains C1
    // nothing changes

    /// The client is in room A and B
    /// Entity is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_entity_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = PeerId::Netcode(111);
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

    // E1 is in rooms A and B
    // C1 is in room A. RoomManager.NetworkVisibility: E1 visible C1
    // C1 leaves room A: E1 loses C1
    // E1 joins room B: E1 gains C1
    // nothing changes

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_client_room() {
        let mut stepper = BevyStepper::default();
        // Client joins room 0 and 1
        let client_id = PeerId::Netcode(111);
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
        let client_id = PeerId::Netcode(111);
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
        let c1 = PeerId::Netcode(TEST_CLIENT_ID_1);
        let c2 = PeerId::Netcode(TEST_CLIENT_ID_2);
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
