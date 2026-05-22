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
# use lightyear_replication::prelude::*;

# let mut app = App::new();
# app.add_plugins(RoomPlugin);
# let room = app.world_mut().resource_mut::<RoomAllocator>().allocate();
# let mut commands = app.world_mut().commands();
// Add the client and entity to the same room: the entity will be
// replicated/visible to clients sharing that room.
let entity = commands.spawn((Replicate::default(), Rooms::single(room))).id();
let client = commands.spawn((ReplicationSender::default(), Rooms::single(room))).id();
```

*/
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::{AppVisibilityExt, VisibilityFilter};
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use fixedbitset::FixedBitSet;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Unique identifier for a room.
///
/// The [`RoomId`] must be allocated via the [`RoomAllocator`] resource.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    bevy_reflect::Reflect,
)]
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

impl<T: Iterator<Item = RoomId>> From<T> for Rooms {
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

    /// Returns true if this entity/client is in the specified room
    pub fn contains_room(&self, room: RoomId) -> bool {
        room.as_usize() < self.rooms.len() && self.rooms.contains(room.as_usize())
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
    type ClientComponent = Self;
    type Scope = Entity;
    fn is_visible(&self, _client: Entity, component: Option<&Self::ClientComponent>) -> bool {
        component.is_some_and(|other| self.rooms.intersection_count(&other.rooms) > 0)
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

    use test_log::test;

    #[test]
    fn room_allocator_returns_distinct_monotonic_ids() {
        let mut allocator = RoomAllocator::default();

        let first = allocator.allocate();
        let second = allocator.allocate();

        assert_eq!(first.as_usize(), 0);
        assert_eq!(second.as_usize(), 1);
    }

    #[test]
    fn rooms_add_remove_and_iterate_memberships() {
        let room_a = RoomId(0);
        let room_b = RoomId(3);
        let mut rooms = Rooms::single(room_a);

        rooms.add_room(room_b);

        assert!(rooms.contains_room(room_a));
        assert!(rooms.contains_room(room_b));
        assert_eq!(
            rooms.rooms().collect::<alloc::vec::Vec<_>>(),
            [room_a, room_b]
        );

        rooms.remove_room(room_a);

        assert!(!rooms.contains_room(room_a));
        assert!(rooms.contains_room(room_b));
        assert_eq!(rooms.rooms().collect::<alloc::vec::Vec<_>>(), [room_b]);
    }

    #[test]
    fn rooms_visibility_filter_requires_shared_room() {
        let sender = Entity::from_bits(1);
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let entity_rooms = Rooms::single(room_a);
        let client_rooms = Rooms::single(room_a);
        let other_client_rooms = Rooms::single(room_b);

        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));
        assert!(!entity_rooms.is_visible(sender, Some(&other_client_rooms)));
        assert!(!entity_rooms.is_visible(sender, None));
    }

    #[test]
    fn rooms_visibility_tracks_client_and_entity_room_moves() {
        let sender = Entity::from_bits(1);
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let mut entity_rooms = Rooms::single(room_a);
        let mut client_rooms = Rooms::single(room_a);

        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));

        client_rooms.remove_room(room_a);
        client_rooms.add_room(room_b);
        assert!(!entity_rooms.is_visible(sender, Some(&client_rooms)));

        entity_rooms.remove_room(room_a);
        entity_rooms.add_room(room_b);
        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));
    }

    #[test]
    fn rooms_visibility_survives_entity_move_when_client_is_in_both_rooms() {
        let sender = Entity::from_bits(1);
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let mut entity_rooms = Rooms::single(room_a);
        let mut client_rooms = Rooms::single(room_a);
        client_rooms.add_room(room_b);

        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));

        entity_rooms.add_room(room_b);
        entity_rooms.remove_room(room_a);
        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));
    }

    #[test]
    fn rooms_visibility_survives_client_move_when_entity_is_in_both_rooms() {
        let sender = Entity::from_bits(1);
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let mut entity_rooms = Rooms::single(room_a);
        let mut client_rooms = Rooms::single(room_a);
        entity_rooms.add_room(room_b);

        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));

        client_rooms.add_room(room_b);
        client_rooms.remove_room(room_a);
        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));
    }

    #[test]
    fn rooms_visibility_is_lost_when_last_shared_room_is_removed() {
        let sender = Entity::from_bits(1);
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let mut entity_rooms = Rooms::single(room_a);
        entity_rooms.add_room(room_b);
        let mut client_rooms = Rooms::single(room_a);
        client_rooms.add_room(room_b);

        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));

        entity_rooms.remove_room(room_a);
        assert!(entity_rooms.is_visible(sender, Some(&client_rooms)));

        entity_rooms.remove_room(room_b);
        assert!(!entity_rooms.is_visible(sender, Some(&client_rooms)));
    }

    #[test]
    fn room_plugin_registers_allocator_resource() {
        let mut app = App::new();

        app.add_plugins(RoomPlugin);

        assert!(app.world().contains_resource::<RoomAllocator>());
    }
}
