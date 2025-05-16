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
use bevy::platform::collections::hash_map::Entry;
use bevy::prelude::*;
use bevy::reflect::Reflect;

use crate::send::ReplicationBufferSet;
use crate::visibility::error::NetworkVisibilityError;
use crate::visibility::immediate::{
    NetworkVisibility, NetworkVisibilityPlugin, VisibilityState
};

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
        mut query: Query<&mut Room>,
    ) -> Result {
        let Ok(mut room) = query.get_mut(trigger.target()) else {
            return Err(NetworkVisibilityError::RoomNotFound(trigger.target()))?;
        };
        match trigger.event() {
            RoomEvent::AddEntity(entity) => {
                trace!("Adding entity {entity:?} to room {room:?}");
                room.clients.iter().for_each(|c| {
                    room_events
                        .events
                        .entry(*entity)
                        .or_default()
                        .gain_visibility(*c);
                });
                room.entities.insert(*entity);
            }
            RoomEvent::RemoveEntity(entity) => {
                trace!("Removing entity {entity:?} from room {room:?}");
                room.clients.iter().for_each(|c| {
                    room_events
                        .events
                        .entry(*entity)
                        .or_default()
                        .lose_visibility(*c);
                });
                room.entities.remove(entity);
            }
            RoomEvent::AddSender(entity) => {
                trace!("Adding sender {entity:?} to room {room:?}");
                room.entities.iter().for_each(|e| {
                    room_events
                        .events
                        .entry(*e)
                        .or_default()
                        .gain_visibility(*entity);
                });
                room.clients.insert(*entity);
            }
            RoomEvent::RemoveSender(entity) => {
                trace!("Removing sender {entity:?} from room {room:?}");
                room.entities.iter().for_each(|e| {
                    room_events
                        .events
                        .entry(*e)
                        .or_default()
                        .lose_visibility(*entity);
                });
                room.clients.remove(entity);
            }
        }
        Ok(())
    }

    pub fn apply_room_events(
        mut commands: Commands,
        mut room_events: ResMut<RoomEvents>,
        mut query: Query<&mut NetworkVisibility>,
    ) {
        // TODO: should we use iter_mut here to keep the allocated NetworkVisibilty?
        room_events
            .events
            .drain(..)
            .for_each(|(entity, mut room_vis)| {
                if let Ok(mut vis) = query.get_mut(entity) {
                    room_vis
                        .clients
                        .drain()
                        .for_each(|(sender, state)| match state {
                            VisibilityState::Gained => vis.gain_visibility(sender),
                            VisibilityState::Lost => vis.lose_visibility(sender),
                            VisibilityState::Maintained => {
                                unreachable!()
                            }
                        });
                } else {
                    trace!("Inserting NetworkVisibility from room visibility: {room_vis:?}");
                    commands
                        .entity(entity)
                        .try_insert(NetworkVisibility::from(room_vis));
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

#[derive(Event, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomEvent {
    AddEntity(Entity),
    RemoveEntity(Entity),
    AddSender(Entity),
    RemoveSender(Entity),
}

#[derive(Default, Debug, Resource)]
pub struct RoomEvents {
    /// List of events that have been triggered by room events
    ///
    /// We cannot apply the [`RoomEvent`]s directly to the entity's [`NetworVisibility`] because
    /// we need to handle concurrent room moves correctly:
    /// if entity E1 and sender A both leave room R1 and join room R2, the visibility should be
    /// unchanged.
    pub(crate) events: EntityIndexMap<RoomVisibility>,
}

#[derive(Debug, Default)]
pub struct RoomVisibility {
    /// List of clients that the entity is currently replicated to.
    /// Will be updated before the other replication systems
    clients: EntityHashMap<VisibilityState>,
}

impl RoomVisibility {
    fn gain_visibility(&mut self, sender: Entity) {
        match self.clients.entry(sender) {
            Entry::Occupied(e) => {
                if *e.get() == VisibilityState::Lost {
                    e.remove();
                }
            }
            Entry::Vacant(e) => {
                e.insert(VisibilityState::Gained);
            }
        }
    }

    fn lose_visibility(&mut self, sender: Entity) {
        match self.clients.entry(sender) {
            Entry::Occupied(e) => {
                if *e.get() == VisibilityState::Gained {
                    e.remove();
                }
            }
            Entry::Vacant(e) => {
                e.insert(VisibilityState::Lost);
            }
        }
    }
}

impl From<RoomVisibility> for NetworkVisibility {
    fn from(value: RoomVisibility) -> Self {
        Self {
            clients: value.clients
        }
    }
}

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<NetworkVisibilityPlugin>() {
            app.add_plugins(NetworkVisibilityPlugin);
        }
        // REFLECT
        app.register_type::<Room>();
        // RESOURCES
        app.init_resource::<RoomEvents>();
        // SETS
        app.configure_sets(
            PostUpdate,
            RoomSet::ApplyRoomEvents.in_set(ReplicationBufferSet::BeforeBuffer),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            Self::apply_room_events.in_set(RoomSet::ApplyRoomEvents),
        );
        // needed in tests to make sure that commands are applied correctly
        #[cfg(test)]
        app.configure_sets(
            PostUpdate,
            RoomSet::ApplyRoomEvents.before(crate::visibility::immediate::VisibilitySet::UpdateVisibility),
        );
        app.add_observer(Self::handle_room_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use test_log::test;

    #[test]
    // entity is in a room
    // we add a client to that room, then we remove it
    fn test_add_remove_client_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        // Client joins room
        let room = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Client leaves room
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveSender(sender), room);
        app.world_mut().flush();
        app.world_mut()
            .run_system_once(RoomPlugin::apply_room_events)
            .ok();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Lost)
        );
    }

    #[test]
    // client is in a room
    // we add an entity to that room, then we remove it
    fn test_add_remove_entity_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        // Entity joins room
        let room = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveEntity(entity), room);
        app.world_mut().flush();
        app.world_mut()
            .run_system_once(RoomPlugin::apply_room_events)
            .ok();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Lost)
        );
    }

    /// The client is in a room with the entity
    /// We move the client and the entity to a different room (client first, then entity)
    /// There should be no change in relevance
    #[test]
    fn test_move_client_entity_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.update();

        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        let room_2 = app.world_mut().spawn(Room::default()).id();
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveEntity(entity), room);
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room_2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room_2);
        app.world_mut().flush();
        app.world_mut()
            .run_system_once(RoomPlugin::apply_room_events)
            .ok();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );
    }

    /// The client is in room A and B
    /// Entity is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_entity_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let room_2 = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room_2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveEntity(entity), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room_2);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );
    }

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_client_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let room_2 = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room_2);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room_2);
        app.update();
        app.world_mut()
            .run_system_once(RoomPlugin::apply_room_events)
            .ok();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );
    }

    /// The entity and client are in room A
    /// Entity,client leave room at the same time
    ///
    /// Entity-Client should lose relevance (not in the same room anymore)
    #[test]
    fn test_client_entity_both_leave_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let entity = Entity::from_raw(1);
        let sender = Entity::from_raw(2);
        app.world_mut()
            .trigger_targets(RoomEvent::AddSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::AddEntity(entity), room);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveSender(sender), room);
        app.world_mut()
            .trigger_targets(RoomEvent::RemoveEntity(entity), room);
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );
    }
}
