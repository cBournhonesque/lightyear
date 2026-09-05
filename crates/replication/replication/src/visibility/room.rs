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
use smallvec::SmallVec;
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};

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
#[derive(Debug, Clone, PartialEq, Component)]
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

/// Marker for hierarchy members whose [`Rooms`] are managed independently.
///
/// Inserted automatically when [`Rooms`] is explicitly set on an entity with
/// [`ReplicateLike`] to a value that differs from its replication root (or
/// removed while the root still has rooms). While present, room changes on the
/// root are no longer mirrored to this entity. Remove it (or the [`Rooms`]
/// component) to re-inherit the root's rooms.
///
/// Inheritance is per-entity from the ultimate [`ReplicateLike`] root: marking
/// one member does not affect its unmarked descendants, which keep following
/// the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component)]
pub struct RoomsOverridden;

/// Mirror `root_rooms` onto `member`, writing only on difference.
///
/// Members without [`RoomsOverridden`] always converge to the ultimate root's
/// rooms. The equality guard is load-bearing: mirroring queues the same
/// insert/remove observers, which must observe a converged state and stop.
fn mirror_member_rooms(
    member: Entity,
    root_rooms: Option<&Rooms>,
    rooms: &Query<&Rooms>,
    commands: &mut Commands,
) {
    match (rooms.get(member).ok(), root_rooms) {
        (Some(current), Some(root)) if current == root => {}
        (_, Some(root)) => {
            commands.entity(member).insert(root.clone());
        }
        (Some(_), None) => {
            commands.entity(member).try_remove::<Rooms>();
        }
        (None, None) => {}
    }
}

/// Mirror the rooms of `root` onto every hierarchy member that has not opted
/// out with [`RoomsOverridden`].
///
/// [`ReplicateLikeChildren`] already contains the whole subtree: every
/// descendant points at the ultimate root, while independently replicated
/// (`Replicate`) sub-roots and their descendants point elsewhere.
fn push_rooms_to_subtree(
    root: Entity,
    root_rooms: Option<&Rooms>,
    children: &Query<&ReplicateLikeChildren>,
    rooms: &Query<&Rooms>,
    overridden: &Query<Has<RoomsOverridden>>,
    commands: &mut Commands,
) {
    let Ok(members) = children.get(root) else {
        return;
    };
    // Copy the entity list: the queued mirrors re-trigger this observer.
    let members: SmallVec<[Entity; 8]> = members.iter().collect();
    for member in members {
        if overridden.get(member).is_ok_and(|overridden| !overridden) {
            mirror_member_rooms(member, root_rooms, rooms, commands);
        }
    }
}

/// Reconcile a member's [`Rooms`] against its replication root.
///
/// This observer never mirrors: it only records intent. Mirror writes always
/// equal the root's rooms at convergence, so equality against the root
/// distinguishes them from explicit overrides:
/// - equal to the root: mirror state, ensure [`RoomsOverridden`] is absent;
/// - different (or present while the root has none): explicit override, insert it.
fn reconcile_member_rooms(
    member: Entity,
    root: Entity,
    rooms: &Query<&Rooms>,
    commands: &mut Commands,
) {
    let root_rooms = rooms.get(root).ok();
    let is_override = match (rooms.get(member).ok(), root_rooms) {
        (Some(current), Some(root)) => current != root,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if is_override {
        commands.entity(member).insert(RoomsOverridden);
    } else {
        commands.entity(member).try_remove::<RoomsOverridden>();
    }
}

/// Keep inherited rooms in sync when [`Rooms`] is inserted or replaced.
/// - on a member with [`ReplicateLike`]: record whether this is an explicit
///   override ([`RoomsOverridden`]) without touching the value;
/// - otherwise it is a root (or standalone): mirror to the unmarked subtree.
///
/// Sender links need no handling: inheriting children carry [`Rooms`], so
/// replicon's own filter observers evaluate them on client changes.
fn propagate_rooms_when_inserted(
    trigger: On<Insert, Rooms>,
    children: Query<&ReplicateLikeChildren>,
    rooms: Query<&Rooms>,
    replicate_like: Query<&ReplicateLike>,
    overridden: Query<Has<RoomsOverridden>>,
    mut commands: Commands,
) {
    if let Ok(like) = replicate_like.get(trigger.entity) {
        reconcile_member_rooms(trigger.entity, like.root, &rooms, &mut commands);
        return;
    }
    let root_rooms = rooms.get(trigger.entity).ok().cloned();
    push_rooms_to_subtree(
        trigger.entity,
        root_rooms.as_ref(),
        &children,
        &rooms,
        &overridden,
        &mut commands,
    );
}

/// Keep inherited rooms in sync when [`Rooms`] is removed.
/// - on a member with [`ReplicateLike`]: removing rooms while the root still
///   has rooms means "replicate everywhere" (public), which is itself an
///   override; removing them while the root has none is mirror state;
/// - otherwise it was a root: the constraint is gone, drop the mirrors on
///   unmarked members.
fn propagate_rooms_when_removed(
    trigger: On<Remove, Rooms>,
    children: Query<&ReplicateLikeChildren>,
    rooms: Query<&Rooms>,
    replicate_like: Query<&ReplicateLike>,
    overridden: Query<Has<RoomsOverridden>>,
    mut commands: Commands,
) {
    if let Ok(like) = replicate_like.get(trigger.entity) {
        if rooms.get(like.root).is_ok() {
            commands.entity(trigger.entity).insert(RoomsOverridden);
        } else {
            commands
                .entity(trigger.entity)
                .try_remove::<RoomsOverridden>();
        }
        return;
    }
    push_rooms_to_subtree(
        trigger.entity,
        None,
        &children,
        &rooms,
        &overridden,
        &mut commands,
    );
}

/// Bring a hierarchy member's [`Rooms`] in line with its replication root.
///
/// - unmarked member without [`Rooms`]: mirror the root's rooms (attach);
/// - unmarked member with differing [`Rooms`]: fresh joins carry user-authored
///   values, so record the override instead of clobbering it. (Stale mirrors
///   from reparenting are force-synced by the hierarchy propagation system,
///   which is the only place that knows the previous root.)
/// - unmarked member with equal [`Rooms`]: mirror state, nothing to do;
/// - marked member: explicit override, never touch.
///
/// `On<Insert>` fires for both fresh inserts and replacements, so this covers
/// reparenting as well: marked members keep their rooms, unmarked members
/// without rooms get mirrored, and unmarked members with rooms are left for
/// the propagation system's force-sync to converge via reconciliation.
fn inherit_rooms_on_replicate_like_added(
    trigger: On<Insert, ReplicateLike>,
    replicate_like: Query<&ReplicateLike>,
    rooms: Query<&Rooms>,
    overridden: Query<Has<RoomsOverridden>>,
    mut commands: Commands,
) {
    let Ok(like) = replicate_like.get(trigger.entity) else {
        return;
    };
    if overridden
        .get(trigger.entity)
        .is_ok_and(|overridden| overridden)
    {
        return;
    }
    match (
        rooms.get(trigger.entity).ok(),
        rooms.get(like.root).ok().cloned(),
    ) {
        (None, root_rooms) => {
            mirror_member_rooms(trigger.entity, root_rooms.as_ref(), &rooms, &mut commands);
        }
        (Some(_), _) => {
            reconcile_member_rooms(trigger.entity, like.root, &rooms, &mut commands);
        }
    }
}

/// Re-inherit the replication root's rooms after [`RoomsOverridden`] is removed.
fn reinherit_rooms_on_override_removed(
    trigger: On<Remove, RoomsOverridden>,
    replicate_like: Query<&ReplicateLike>,
    rooms: Query<&Rooms>,
    mut commands: Commands,
) {
    let Ok(like) = replicate_like.get(trigger.entity) else {
        return;
    };
    let root_rooms = rooms.get(like.root).ok().cloned();
    mirror_member_rooms(trigger.entity, root_rooms.as_ref(), &rooms, &mut commands);
}

/// Plugin used to handle interest management via [`Rooms`].
///
/// Members of a replication hierarchy ([`ReplicateLike`]) inherit their
/// root's rooms, so only the root needs to be added to rooms. A member with
/// explicitly set [`Rooms`] keeps them: the explicit write is recorded with
/// [`RoomsOverridden`] and root changes no longer propagate to it. Remove the
/// marker (or the [`Rooms`] component) to re-inherit the root's rooms.
///
/// Explicitly setting a member's rooms to the root's current value is
/// equivalent to inheriting.
#[derive(Default)]
pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FilterRegistry>()
            .init_resource::<ReplicationRegistry>()
            .init_resource::<RoomAllocator>();
        app.add_visibility_filter::<Rooms>();
        app.add_observer(propagate_rooms_when_inserted);
        app.add_observer(propagate_rooms_when_removed);
        app.add_observer(inherit_rooms_on_replicate_like_added);
        app.add_observer(reinherit_rooms_on_override_removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::{HierarchyPlugin, HierarchySendPlugin};
    use crate::prelude::Replicate;
    use alloc::vec;
    use bevy_replicon::prelude::{AuthMethod, RepliconSharedPlugin};
    use bevy_replicon::server::ServerPlugin;

    use test_log::test;

    /// App with hierarchy propagation and room inheritance, without networking.
    /// Room inheritance is component-level, so plain component assertions suffice.
    fn rooms_hierarchy_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy_state::app::StatesPlugin);
        app.init_resource::<bevy_time::Time>();
        app.add_plugins(RepliconSharedPlugin {
            auth_method: AuthMethod::None,
        });
        app.add_plugins(ServerPlugin::default());
        app.add_plugins(HierarchySendPlugin::<ChildOf>::default());
        app.add_plugins(HierarchyPlugin);
        app.add_plugins(RoomPlugin);
        app
    }

    /// Flush the propagation system and all queued observer commands.
    fn sync(app: &mut App) {
        for _ in 0..3 {
            app.update();
        }
    }

    fn rooms_of(app: &App, entity: Entity) -> Option<Rooms> {
        app.world().get::<Rooms>(entity).cloned()
    }

    fn is_overridden(app: &App, entity: Entity) -> bool {
        app.world().get::<RoomsOverridden>(entity).is_some()
    }

    #[test]
    fn members_mirror_root_rooms_on_attach() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let child = app.world_mut().spawn(ChildOf(root)).id();
        sync(&mut app);

        assert_eq!(rooms_of(&app, child), Some(Rooms::single(room_a)));
        assert!(!is_overridden(&app, child));
    }

    #[test]
    fn members_with_explicit_rooms_keep_override() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let child = app
            .world_mut()
            .spawn((ChildOf(root), Rooms::single(room_b)))
            .id();
        sync(&mut app);

        assert_eq!(rooms_of(&app, child), Some(Rooms::single(room_b)));
        assert!(is_overridden(&app, child));

        // ...and an explicitly set value equal to the root's counts as inheriting.
        let other = app
            .world_mut()
            .spawn((ChildOf(root), Rooms::single(room_a)))
            .id();
        sync(&mut app);
        assert_eq!(rooms_of(&app, other), Some(Rooms::single(room_a)));
        assert!(!is_overridden(&app, other));
    }

    #[test]
    fn root_changes_reach_plain_members_but_not_overrides() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let room_c = RoomId(2);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        let over = app
            .world_mut()
            .spawn((ChildOf(root), Rooms::single(room_b)))
            .id();
        sync(&mut app);

        app.world_mut()
            .entity_mut(root)
            .insert(Rooms::single(room_c));
        sync(&mut app);

        assert_eq!(rooms_of(&app, plain), Some(Rooms::single(room_c)));
        assert!(!is_overridden(&app, plain));
        assert_eq!(rooms_of(&app, over), Some(Rooms::single(room_b)));
        assert!(is_overridden(&app, over));
    }

    #[test]
    fn root_removal_drops_mirrors_but_keeps_overrides() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        let over = app
            .world_mut()
            .spawn((ChildOf(root), Rooms::single(room_b)))
            .id();
        sync(&mut app);

        app.world_mut().entity_mut(root).remove::<Rooms>();
        sync(&mut app);

        assert_eq!(rooms_of(&app, plain), None);
        assert!(!is_overridden(&app, plain));
        assert_eq!(rooms_of(&app, over), Some(Rooms::single(room_b)));
        assert!(is_overridden(&app, over));
    }

    #[test]
    fn member_removal_while_root_has_rooms_is_an_override() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let child = app.world_mut().spawn(ChildOf(root)).id();
        sync(&mut app);
        assert_eq!(rooms_of(&app, child), Some(Rooms::single(room_a)));

        // Going filter-less ("replicate everywhere") while the root is
        // room-bound is itself an override: it must stick.
        app.world_mut().entity_mut(child).remove::<Rooms>();
        sync(&mut app);
        assert_eq!(rooms_of(&app, child), None);
        assert!(is_overridden(&app, child));

        // A later root change must not touch it.
        app.world_mut()
            .entity_mut(root)
            .insert(Rooms::single(RoomId(1)));
        sync(&mut app);
        assert_eq!(rooms_of(&app, child), None);
        assert!(is_overridden(&app, child));
    }

    #[test]
    fn removing_override_reinherits_root_rooms() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let child = app
            .world_mut()
            .spawn((ChildOf(root), Rooms::single(room_b)))
            .id();
        sync(&mut app);
        assert!(is_overridden(&app, child));

        app.world_mut()
            .entity_mut(child)
            .remove::<RoomsOverridden>();
        sync(&mut app);

        assert_eq!(rooms_of(&app, child), Some(Rooms::single(room_a)));
        assert!(!is_overridden(&app, child));
    }

    #[test]
    fn reparent_reconciles_stale_mirrors_but_keeps_overrides() {
        let mut app = rooms_hierarchy_app();
        let room_a = RoomId(0);
        let room_b = RoomId(1);
        let room_c = RoomId(2);
        let root_a = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_a)))
            .id();
        let root_c = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), Rooms::single(room_c)))
            .id();
        let plain = app.world_mut().spawn(ChildOf(root_a)).id();
        let over = app
            .world_mut()
            .spawn((ChildOf(root_a), Rooms::single(room_b)))
            .id();
        sync(&mut app);
        assert_eq!(rooms_of(&app, plain), Some(Rooms::single(room_a)));

        app.world_mut().entity_mut(plain).insert(ChildOf(root_c));
        app.world_mut().entity_mut(over).insert(ChildOf(root_c));
        sync(&mut app);

        // Stale mirror follows the new root; the genuine override travels.
        assert_eq!(rooms_of(&app, plain), Some(Rooms::single(room_c)));
        assert!(!is_overridden(&app, plain));
        assert_eq!(rooms_of(&app, over), Some(Rooms::single(room_b)));
        assert!(is_overridden(&app, over));
    }

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
