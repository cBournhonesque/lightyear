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
# use bevy_app::App;
# use bevy_ecs::entity::Entity;
# use lightyear_replication::prelude::*;

# let mut app = App::new();
# let mut commands = app.world_mut().commands();
// create a new room
let room = commands.spawn(Room::default()).id();

let entity = commands.spawn(Replicate::default()).id();
let client = commands.spawn(ReplicationSender::default()).id();

// add the client and entity to the same room: the entity will be replicated/visible to the client
commands.trigger(RoomEvent { target: RoomTarget::AddEntity(entity), room });
commands.trigger(RoomEvent { target: RoomTarget::AddSender(client), room });
```

*/

use crate::send::plugin::ReplicationBufferSystems;
use crate::visibility::error::NetworkVisibilityError;
use crate::visibility::immediate::{NetworkVisibility, NetworkVisibilityPlugin, VisibilityState};
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::entity::{EntityHashMap, EntityHashSet, EntityIndexMap};
use bevy_ecs::prelude::*;
use bevy_platform::collections::hash_map::Entry;
use bevy_reflect::Reflect;
use lightyear_connection::client::Disconnected;
#[allow(unused_imports)]
use tracing::{info, trace};

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
    /// Pop the disconnected client from all rooms
    fn handle_disconnect(
        trigger: On<Add, Disconnected>,
        mut query: Query<&mut Room>,
        mut room_events: ResMut<RoomEvents>,
    ) {
        query.iter_mut().for_each(|mut room| {
            room.clients.remove(&trigger.entity);
        });
        room_events.shared_counts.remove(&trigger.entity);
    }

    fn handle_room_event(
        trigger: On<RoomEvent>,
        mut room_events: ResMut<RoomEvents>,
        mut room_query: Query<&mut Room>,
    ) -> Result {
        let room_entity = trigger.room;
        let Ok(mut room) = room_query.get_mut(room_entity) else {
            return Err(NetworkVisibilityError::RoomNotFound(room_entity))?;
        };

        // enable partial borrows
        let room_events = room_events.as_mut();
        match &trigger.event().target {
            RoomTarget::AddEntity(entity) => {
                if room.entities.insert(*entity) {
                    trace!("Adding entity {entity:?} to room {room_entity:?}");
                    for client in room.clients.iter() {
                        let count = room_events
                            .shared_counts
                            .entry(*client)
                            .or_default()
                            .entry(*entity)
                            .or_default();
                        if *count == 0 {
                            room_events
                                .events
                                .entry(*entity)
                                .or_default()
                                .gain_visibility(*client);
                        }
                        *count = count.saturating_add(1);
                    }
                }
            }
            RoomTarget::RemoveEntity(entity) => {
                // Only run if the entity was actually in the room
                if room.entities.remove(entity) {
                    trace!("Removing entity {entity:?} from room {room_entity:?}");
                    for client in room.clients.iter() {
                        let count = room_events
                            .shared_counts
                            .entry(*client)
                            .or_default()
                            .entry(*entity)
                            .or_default();
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            room_events
                                .events
                                .entry(*entity)
                                .or_default()
                                .lose_visibility(*client);
                        }
                    }
                }
            }
            RoomTarget::AddSender(client) => {
                if room.clients.insert(*client) {
                    trace!("Adding sender {client:?} to room {room_entity:?}");
                    for entity in room.entities.iter() {
                        let count = room_events
                            .shared_counts
                            .entry(*client)
                            .or_default()
                            .entry(*entity)
                            .or_default();
                        if *count == 0 {
                            room_events
                                .events
                                .entry(*entity)
                                .or_default()
                                .gain_visibility(*client);
                        }
                        *count = count.saturating_add(1);
                    }
                }
            }
            RoomTarget::RemoveSender(client) => {
                if room.clients.remove(client) {
                    trace!("Removing sender {client:?} from room {room_entity:?}");
                    for entity in room.entities.iter() {
                        let count = room_events
                            .shared_counts
                            .entry(*client)
                            .or_default()
                            .entry(*entity)
                            .or_default();
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            room_events
                                .events
                                .entry(*entity)
                                .or_default()
                                .lose_visibility(*client);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_room_events(
        mut commands: Commands,
        mut room_events: ResMut<RoomEvents>,
        mut query: Query<&mut NetworkVisibility>,
    ) {
        // TODO: should we use iter_mut here to keep the allocated NetworkVisibility?
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
                    trace!(
                        ?entity,
                        "Inserting NetworkVisibility from room visibility: {room_vis:?}"
                    );
                    commands
                        .entity(entity)
                        .try_insert(NetworkVisibility::from(room_vis));
                }
            });
    }
}

#[deprecated(note = "Use RoomSystems instead")]
pub type RoomSet = RoomSystems;

/// System sets related to Rooms
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomSystems {
    /// Update the [`NetworkVisibility`] components based on room memberships
    ApplyRoomEvents,
}

/// Event that can be triggered to modify the entities/peers that belong in a [`Room`]
#[derive(EntityEvent, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct RoomEvent {
    #[event_target]
    pub room: Entity,
    pub target: RoomTarget,
}

/// Identifies the entity that will be added or removed in the room
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RoomTarget {
    AddEntity(Entity),
    RemoveEntity(Entity),
    AddSender(Entity),
    RemoveSender(Entity),
}

#[derive(Default, Debug, Resource)]
pub(crate) struct RoomEvents {
    /// List of events that have been triggered by room events.
    /// Keyed by the [`Room`] entity
    ///
    /// We cannot apply the [`RoomEvent`]s directly to the entity's [`NetworkVisibility`] because
    /// we need to handle concurrent room moves correctly:
    /// if entity E1 and sender A both leave room R1 and join room R2, the visibility should be
    /// unchanged.
    pub(crate) events: EntityIndexMap<RoomVisibility>,
    /// Count of the number of rooms shared by a client and a sender
    // client -> entity -> u8
    shared_counts: EntityHashMap<EntityHashMap<u8>>,
}

#[derive(Debug, Default)]
pub(crate) struct RoomVisibility {
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
            clients: value.clients,
        }
    }
}

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<NetworkVisibilityPlugin>() {
            app.add_plugins(NetworkVisibilityPlugin);
        }
        // REFLECT
        // RESOURCES
        app.init_resource::<RoomEvents>();
        // SETS
        app.configure_sets(
            PostUpdate,
            RoomSystems::ApplyRoomEvents.in_set(ReplicationBufferSystems::BeforeBuffer),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            Self::apply_room_events.in_set(RoomSystems::ApplyRoomEvents),
        );
        // needed in tests to make sure that commands are applied correctly
        #[cfg(test)]
        app.configure_sets(
            PostUpdate,
            RoomSystems::ApplyRoomEvents
                .before(crate::visibility::immediate::VisibilitySystems::UpdateVisibility),
        );
        app.add_observer(Self::handle_room_event);
        app.add_observer(Self::handle_disconnect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::prelude::{Replicate, ReplicationSender};
    use alloc::vec;
    use bevy_ecs::system::RunSystemOnce;
    use test_log::test;

    #[test]
    // entity is in a room
    // we add a client to that room, then we remove it
    fn test_add_remove_client_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        // Client joins room
        let room = app.world_mut().spawn(Room::default()).id();
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            // VisibilityGained -> Replicate -> Maintained
            Some(&VisibilityState::Maintained)
        );

        // Client leaves room
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveSender(sender),
            room,
        });
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
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            // VisibilityGained -> Replicate -> Maintained
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room,
        });
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
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.update();

        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity/client move to a different room
        let room_2 = app.world_mut().spawn(Room::default()).id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room: room_2,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room: room_2,
        });
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
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room: room_2,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity moves from room 1 to 2 (sender belongs in both)
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room: room_2,
        });
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

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_client_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let room_2 = app.world_mut().spawn(Room::default()).id();
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room: room_2,
        });
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room: room_2,
        });
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

    /// The entity and client are in room A
    /// Entity,client leave room at the same time
    ///
    /// Entity-Client should lose relevance (not in the same room anymore)
    #[test]
    fn test_client_entity_both_leave_room() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity/client leaves room
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room,
        });
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

    /// Client and entity are both in rooms A and B.
    /// Entity leaves room A: they should still remain relevant since they are both in room B.
    /// Entity leaves room B: now the visibility should be lost
    #[test]
    fn test_client_entity_multiple_shared_rooms() {
        let mut app = App::new();
        app.add_plugins(RoomPlugin);

        let room = app.world_mut().spawn(Room::default()).id();
        let room_2 = app.world_mut().spawn(Room::default()).id();
        let sender = app.world_mut().spawn(ReplicationSender::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                NetworkVisibility::default(),
                Replicate::manual(vec![sender]),
            ))
            .id();
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddSender(sender),
            room: room_2,
        });
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::AddEntity(entity),
            room: room_2,
        });

        app.update();

        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // Entity leaves room 1
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room,
        });
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

        // Entity leaves room 2
        app.world_mut().trigger(RoomEvent {
            target: RoomTarget::RemoveEntity(entity),
            room: room_2,
        });
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
}
