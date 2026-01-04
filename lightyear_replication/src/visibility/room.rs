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
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::prelude::{AppVisibilityExt, VisibilityFilter};
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use fixedbitset::FixedBitSet;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Unique identifier for a room.
///
/// The [`RoomId`] must be allocated via the [`RoomAllocator`] resource.
#[derive(Debug, Clone, Copy)]
pub struct RoomId(u16);

impl RoomId {
    /// Returns the underlying usize value of the RoomId
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}
impl From<RoomId> for usize {
    fn from(value: RoomId) -> Self {
        value.0 as usize
    }
}

#[derive(Debug, Resource)]
pub struct RoomAllocator {
    next_id: RoomId,
}

impl Default for RoomAllocator {
    fn default() -> Self {
        Self { next_id: RoomId(0) }
    }
}

impl RoomAllocator {
    pub fn allocate(&mut self) -> RoomId {
        let id = self.next_id;
        self.next_id = RoomId(self.next_id.0.checked_add(1).expect("RoomId overflow"));
        id
    }
}



/// A [`Rooms`] is a component that represents the list of rooms that the entity or client belongs to.
///
/// It is used to manage interest management via rooms.
/// The entity will be replicated to all clients that share at least one room with the entity.
///
/// The room ids must be allocated via the [`RoomAllocator`] resource.
#[derive(Debug, Component)]
#[component(immutable)]
pub struct Rooms {
    /// list of rooms that the entity/client belongs to
    rooms: FixedBitSet,
}

impl<T: Iterator<Item=RoomId>> From<T> for Rooms{
    fn from(value: T) -> Self {
        let mut rooms = Self::default();
        for room in value {
            rooms.add_room(room);
        }
        rooms
    }
}

impl Rooms {
    pub fn single(room: RoomId) -> Self {
        let mut rooms = FixedBitSet::with_capacity(room.as_usize() + 1);
        rooms.set(room.as_usize(), true);
        Self { rooms }
    }

    pub fn rooms(&self) -> impl Iterator<Item = RoomId> + '_ {
        self.rooms.ones().map(|index| RoomId(index as u16))
    }

    /// Adds an extra room to the list of rooms
    pub fn add_room(&mut self, room: RoomId) {
        if room.as_usize() >= self.rooms.len() {
            self.rooms.grow(room.as_usize() + 1);
        }
        self.rooms.set(room.as_usize(), true);
    }

    /// Removes the entity/client from the specified room
    pub fn remove_room(&mut self, room: RoomId) {
        if room.as_usize() < self.rooms.len() {
            self.rooms.set(room.as_usize(), false);
        }
    }
}

impl Default for Rooms {
    fn default() -> Self {
        Self {
            rooms: FixedBitSet::with_capacity(1),
        }
    }
}

impl VisibilityFilter for Rooms {
    type Scope = Entity;
    fn is_visible(&self, other: &Self) -> bool {
        self.rooms.intersection_count(&other.rooms) > 0
    }
}

/// Plugin used to handle interest managements via [`Room`]s
#[derive(Default)]
pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FilterRegistry>()
            .init_resource::<ReplicationRegistry>()
            .init_resource::<RoomAllocator>();
        app.add_visibility_filter::<Rooms>();
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
        let room = 0;
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            // VisibilityGained -> Replicate -> Maintained
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Lost
        );
    }

    #[test]
    // client is in a room
    // we add an entity to that room, then we remove it
    fn test_add_remove_entity_room() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            // VisibilityGained -> Replicate -> Maintained
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Lost
        );
    }

    /// The client is in a room with the entity
    /// We move the client and the entity to a different room (client first, then entity)
    /// There should be no change in relevance
    #[test]
    fn test_move_client_entity_room() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
        );
    }

    /// The client is in room A and B
    /// Entity is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_entity_room() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
        );
    }

    /// The entity is in room A and B
    /// Client is in room A and moves to room B
    /// There should be no change in relevance
    #[test]
    fn test_move_client_room() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
        );
    }

    /// The entity and client are in room A
    /// Entity,client leave room at the same time
    ///
    /// Entity-Client should lose relevance (not in the same room anymore)
    #[test]
    fn test_client_entity_both_leave_room() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Lost
        );
    }

    /// Client and entity are both in rooms A and B.
    /// Entity leaves room A: they should still remain relevant since they are both in room B.
    /// Entity leaves room B: now the visibility should be lost
    #[test]
    fn test_client_entity_multiple_shared_rooms() {
        let mut app = App::new();
        app.init_resource::<ReplicableRootEntities>();
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
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
                .get::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Lost
        );
    }
}
