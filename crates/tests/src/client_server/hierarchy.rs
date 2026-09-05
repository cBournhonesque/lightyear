//! Check various replication scenarios between 2 peers only

use crate::stepper::*;
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_core::prediction::Predicted;
use lightyear_messages::MessageManager;
use lightyear_replication::visibility::immediate::VisibilityExt;
use test_log::test;

// TODO:
// - remove Replicate from a parent: child should get despawned
// -

/// Add a child to a replicated Entity: the child should be replicated
/// and the ChildOf component should be present on the replicated entity
#[test]
fn test_spawn_with_child() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(2);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_entity),))
        .id();
    stepper.frame_step(2);
    let client_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .expect("entity is not present in entity map");
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<ChildOf>(client_child)
            .unwrap()
            .parent(),
        client_entity
    );
}

#[test]
fn test_despawn_with_child() {}

fn setup_hierarchy() -> (ClientServerStepper, Entity, Entity, Entity) {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let grandparent = stepper.server_app.world_mut().spawn_empty().id();
    let parent = stepper
        .server_app
        .world_mut()
        .spawn(ChildOf(grandparent))
        .id();
    let child = stepper.server_app.world_mut().spawn(ChildOf(parent)).id();
    (stepper, grandparent, parent, child)
}

#[test]
fn test_hierarchy_replication() {
    let (mut stepper, grandparent, parent, child) = setup_hierarchy();

    let replicate = Replicate::manual(vec![stepper.client_of_entities[0]]);
    // disable propagation to the child, so the child won't have ReplicateLike or RelationshipSync
    stepper
        .server_app
        .world_mut()
        .entity_mut(child)
        .insert(DisableReplicateHierarchy);
    // add Replicate, which should propagate the RelationshipSync and ReplicateLike through the hierarchy
    stepper
        .server_app
        .world_mut()
        .entity_mut(grandparent)
        .insert(replicate);
    stepper.frame_step(2);

    // check that the parent got replicated, along with the hierarchy information
    let client_grandparent = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(grandparent)
        .expect("entity is not present in entity map");
    let client_parent = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(parent)
        .expect("entity is not present in entity map");

    let (client_parent, client_parent_component) = stepper
        .client_app()
        .world_mut()
        .query::<(Entity, &ChildOf)>()
        .single(stepper.client_app().world())
        .unwrap();

    assert_eq!(client_parent_component.parent(), client_grandparent);

    // TODO: check that the parent/grandparent have the same ReplicationGroupId

    // check that the child did not get replicated
    assert!(
        stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child)
            .is_none()
    );

    // remove the hierarchy on the sender side
    stepper
        .server_app
        .world_mut()
        .entity_mut(parent)
        .remove::<ChildOf>();
    let replicate_like = stepper.server_app.world_mut().get::<ReplicateLike>(parent);
    stepper.frame_step(2);

    // 1. make sure that the parent has been removed on the receiver side
    assert_eq!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(client_parent)
            .get::<ChildOf>(),
        None,
    );
    assert!(
        stepper
            .client_app()
            .world_mut()
            .entity_mut(client_grandparent)
            .get::<Children>()
            .is_none()
    );
}

/// https://github.com/cBournhonesque/lightyear/issues/649
/// P1 with child C1
/// If you add a new client to the replication target of P1, then both
/// P1 and C1 should be replicated to the new client.
/// (the issue says that only P1 was replicated)
#[test]
fn test_new_client_is_added_to_parent() {}

/// https://github.com/cBournhonesque/lightyear/issues/547
/// Test that when a new child is added to a parent
/// the child is also replicated to the remote
#[test]
fn test_propagate_hierarchy_new_child() {}

#[test]
fn test_child_overrides_prediction_target() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step_server_first(1);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    let server_child = stepper
        .server_app
        .world_mut()
        // the child should not be interpolated; it has InterpolationTarget, which takes precedence over the one
        // from the root entity
        .spawn((ChildOf(server_entity), InterpolationTarget::manual(vec![])))
        .id();
    stepper.frame_step_server_first(1);
    let client_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .expect("entity is not present in entity map");
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<ChildOf>(client_child)
            .unwrap()
            .parent(),
        client_entity
    );
    // the parent is interpolated, but not the child
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Interpolated>(client_child)
            .is_none()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Interpolated>(client_entity)
            .is_some()
    );
}

/// Test that lose_visibility on a parent propagates to ReplicateLike children.
#[test]
fn test_hierarchy_visibility_propagates_to_children() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(1.0),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_parent), crate::protocol::CompFull(2.0)))
        .id();
    stepper.frame_step(2);

    // Both parent and child should be visible to both clients initially
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_some(),
        "client 1 should see parent"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_some(),
        "client 1 should see child"
    );

    // Lose visibility for ONLY the parent on client 1
    let sender_1 = stepper.client_of_entities[1];
    stepper
        .server_app
        .world_mut()
        .commands()
        .lose_visibility(server_parent, sender_1);
    stepper.frame_step(2);

    // Parent should be hidden for client 1
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_none(),
        "client 1 should not see parent after lose_visibility"
    );

    // Child should also be hidden — lose_visibility propagates through the ChildOf hierarchy
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_none(),
        "child should not be visible after lose_visibility on parent"
    );
}

/// Test that a child spawned AFTER lose_visibility on the parent inherits
/// the parent's hidden state instead of being visible by default.
#[test]
fn test_hierarchy_visibility_late_child_stays_hidden() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(1.0),
        ))
        .id();
    stepper.frame_step(2);

    // Both clients see the parent initially
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_some(),
        "client 1 should see parent"
    );

    // Hide the parent from client 1 before the child exists
    let sender_1 = stepper.client_of_entities[1];
    stepper
        .server_app
        .world_mut()
        .commands()
        .lose_visibility(server_parent, sender_1);
    stepper.frame_step(2);
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_none(),
        "client 1 should not see parent after lose_visibility"
    );

    // Spawn the child after the parent was hidden
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_parent), crate::protocol::CompFull(2.0)))
        .id();
    stepper.frame_step(2);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ReplicateLike>(server_child)
            .unwrap()
            .root,
        server_parent
    );
    // Client 0 (parent still visible) should see the late child
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_some(),
        "client 0 should see late-spawned child"
    );
    // Client 1 (parent hidden) should NOT see the late child
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_none(),
        "late-spawned child should inherit lose_visibility from parent"
    );
}

/// Test that a child with explicitly set visibility keeps it (override):
/// direct writes on a member are recorded with `VisibilityOverridden` and
/// later parent changes no longer propagate to it. Removing the marker
/// re-inherits, and an overridden-visible child replicates while hidden.
#[test]
fn test_hierarchy_visibility_child_override() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(1.0),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_parent), crate::protocol::CompFull(2.0)))
        .id();
    stepper.frame_step(2);

    let is_visible = |stepper: &ClientServerStepper, entity: Entity| {
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(entity)
            .is_some()
    };
    let is_marked = |stepper: &ClientServerStepper| {
        stepper
            .server_app
            .world()
            .get::<VisibilityOverridden>(server_child)
            .is_some()
    };
    let sender_1 = stepper.client_of_entities[1];

    assert!(is_visible(&stepper, server_parent));
    assert!(is_visible(&stepper, server_child));

    // hide the child directly: it stays hidden across parent pushes
    stepper
        .server_app
        .world_mut()
        .commands()
        .lose_visibility(server_child, sender_1);
    stepper.frame_step(2);
    assert!(is_visible(&stepper, server_parent));
    assert!(!is_visible(&stepper, server_child));
    assert!(is_marked(&stepper), "direct write should mark the child");

    stepper
        .server_app
        .world_mut()
        .commands()
        .gain_visibility(server_parent, sender_1);
    stepper.frame_step(2);
    assert!(is_visible(&stepper, server_parent));
    assert!(
        !is_visible(&stepper, server_child),
        "parent push should skip the overridden child"
    );

    // removing the marker re-inherits the parent's (visible) state
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_child)
        .remove::<VisibilityOverridden>();
    stepper.frame_step(2);
    assert!(is_visible(&stepper, server_child));
    assert!(!is_marked(&stepper));

    // hiding the parent hides the child again (it follows once more)
    stepper
        .server_app
        .world_mut()
        .commands()
        .lose_visibility(server_parent, sender_1);
    stepper.frame_step(2);
    assert!(!is_visible(&stepper, server_parent));
    assert!(!is_visible(&stepper, server_child));

    // widening: the child replicates while the parent stays hidden
    stepper
        .server_app
        .world_mut()
        .commands()
        .gain_visibility(server_child, sender_1);
    stepper.frame_step(2);
    assert!(!is_visible(&stepper, server_parent));
    assert!(
        is_visible(&stepper, server_child),
        "overridden child should replicate while the parent is hidden"
    );
    assert!(is_marked(&stepper));
}

/// Test that a child with its own `Replicate` is still linked (`ReplicateLike`)
/// and inherits rooms/visibility, while its own replication config acts as an
/// override: here the child's target includes a client the rooms exclude.
#[test]
fn test_hierarchy_replicate_child_overrides_target_inherits_rooms() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let room_a = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();
    let room_b = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();

    // client 0 is in room A, client 1 is in room B
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[0])
        .insert(Rooms::single(room_a));
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[1])
        .insert(Rooms::single(room_b));

    let client_0_id = stepper
        .client_of(0)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;

    // parent replicates to client 0 only and lives in room A
    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::Single(client_0_id)),
            crate::protocol::CompFull(1.0),
            Rooms::single(room_a),
        ))
        .id();
    // child overrides the target (all clients) but inherits rooms
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(2.0),
        ))
        .id();
    stepper.frame_step(2);

    // linked, own config kept, rooms mirrored from the parent
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ReplicateLike>(server_child)
            .map(|like| like.root),
        Some(server_parent)
    );
    assert_eq!(
        stepper.server_app.world().get::<Replicate>(server_child),
        Some(&Replicate::to_clients(NetworkTarget::All))
    );
    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_child),
        Some(&Rooms::single(room_a))
    );
    assert!(
        stepper
            .server_app
            .world()
            .get::<RoomsOverridden>(server_child)
            .is_none()
    );

    let is_visible = |stepper: &ClientServerStepper, client: usize, entity: Entity| {
        stepper
            .client(client)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(entity)
            .is_some()
    };
    // client 0 sees both; client 1 sees neither: the child's own target
    // includes it, but the inherited rooms exclude it
    assert!(is_visible(&stepper, 0, server_parent));
    assert!(is_visible(&stepper, 0, server_child));
    assert!(!is_visible(&stepper, 1, server_parent));
    assert!(
        !is_visible(&stepper, 1, server_child),
        "inherited rooms should win over the child's own target"
    );
}

/// Test that a child with ReplicateLike follows the parent's room visibility,
/// even though the child itself has no Rooms component.
#[test]
fn test_hierarchy_rooms_propagate_to_children() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let room_a = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();
    let room_b = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();

    // client 0 is in room A, client 1 is in room B
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[0])
        .insert(Rooms::single(room_a));
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[1])
        .insert(Rooms::single(room_b));

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(1.0),
            Rooms::single(room_a),
        ))
        .id();
    // the child has no Rooms of its own; it should use the root's rooms
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_parent), crate::protocol::CompFull(2.0)))
        .id();
    stepper.frame_step(2);

    // ReplicateLike propagation should have linked the child to the parent
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ReplicateLike>(server_child)
            .unwrap()
            .root,
        server_parent
    );

    // parent is in room A: only client 0 sees parent and child
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_some(),
        "client 0 should see parent in room A"
    );
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_some(),
        "client 0 should see child of parent in room A"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_none(),
        "client 1 should not see parent in room A"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_none(),
        "client 1 should not see child of parent in room A"
    );

    // move the parent to room B: visibility should flip for both parent and child
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_parent)
        .insert(Rooms::single(room_b));
    stepper.frame_step(2);

    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_none(),
        "client 0 should not see parent after it moved to room B"
    );
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_none(),
        "client 0 should not see child after parent moved to room B"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_some(),
        "client 1 should see parent after it moved to room B"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_some(),
        "client 1 should see child after parent moved to room B"
    );

    // move client 0 into room B as well: it should see parent and child again
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[0])
        .insert(Rooms::single(room_b));
    stepper.frame_step(2);

    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_parent)
            .is_some(),
        "client 0 should see parent after joining room B"
    );
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_child)
            .is_some(),
        "client 0 should see child after joining room B"
    );
}

/// Test that a child with explicitly set rooms keeps them (override):
/// plain children mirror the root, but an overridden child replicates
/// independently and survives root room changes. Removing the override
/// re-inherits the root's rooms.
#[test]
fn test_hierarchy_rooms_child_override() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let room_a = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();
    let room_b = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();
    let room_c = stepper
        .server_app
        .world_mut()
        .resource_mut::<RoomAllocator>()
        .allocate();

    // client 0 is in room A, client 1 is in room B, nobody is in room C
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[0])
        .insert(Rooms::single(room_a));
    stepper
        .server_app
        .world_mut()
        .entity_mut(stepper.client_of_entities[1])
        .insert(Rooms::single(room_b));

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            crate::protocol::CompFull(1.0),
            Rooms::single(room_a),
        ))
        .id();
    // plain child: rooms are mirrored on attach
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((ChildOf(server_parent), crate::protocol::CompFull(2.0)))
        .id();
    // child spawned with different rooms: kept as an explicit override
    let server_other_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            crate::protocol::CompFull(3.0),
            Rooms::single(room_b),
        ))
        .id();
    stepper.frame_step(2);

    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_child),
        Some(&Rooms::single(room_a)),
        "plain child should mirror the root's rooms"
    );
    assert!(
        stepper
            .server_app
            .world()
            .get::<RoomsOverridden>(server_child)
            .is_none(),
        "plain child should not be marked as overridden"
    );
    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_other_child),
        Some(&Rooms::single(room_b)),
        "explicit child rooms should be kept"
    );
    assert!(
        stepper
            .server_app
            .world()
            .get::<RoomsOverridden>(server_other_child)
            .is_some(),
        "explicit child rooms should be marked as overridden"
    );

    let is_visible = |stepper: &ClientServerStepper, client: usize, entity: Entity| {
        stepper
            .client(client)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(entity)
            .is_some()
    };
    // parent and plain child follow room A; the override follows room B
    assert!(is_visible(&stepper, 0, server_parent));
    assert!(is_visible(&stepper, 0, server_child));
    assert!(!is_visible(&stepper, 0, server_other_child));
    assert!(!is_visible(&stepper, 1, server_parent));
    assert!(!is_visible(&stepper, 1, server_child));
    assert!(is_visible(&stepper, 1, server_other_child));

    // move the parent to room C: the plain child follows, the override survives
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_parent)
        .insert(Rooms::single(room_c));
    stepper.frame_step(2);

    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_child),
        Some(&Rooms::single(room_c)),
        "plain child should follow the root's new rooms"
    );
    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_other_child),
        Some(&Rooms::single(room_b)),
        "override should survive the root's room change"
    );
    assert!(!is_visible(&stepper, 0, server_parent));
    assert!(!is_visible(&stepper, 0, server_child));
    assert!(!is_visible(&stepper, 1, server_parent));
    assert!(!is_visible(&stepper, 1, server_child));
    assert!(
        is_visible(&stepper, 1, server_other_child),
        "override child should stay visible in room B"
    );

    // removing the override re-inherits the root's rooms
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_other_child)
        .remove::<RoomsOverridden>();
    stepper.frame_step(2);

    assert_eq!(
        stepper.server_app.world().get::<Rooms>(server_other_child),
        Some(&Rooms::single(room_c)),
        "unmarked child should re-inherit the root's rooms"
    );
    assert!(
        !is_visible(&stepper, 1, server_other_child),
        "re-inherited child should follow room C"
    );
}

/// Switching the root's `PredictionTarget` propagates to hierarchy members:
/// the child follows the new target server-side and gains/loses `Predicted`
/// on the client.
#[test]
fn test_root_prediction_target_switch_propagates_to_child() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let client_id = stepper
        .client_of(0)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;

    let server_root = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn(ChildOf(server_root))
        .id();
    stepper.frame_step_server_first(4);
    let client_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .expect("child should be replicated");

    // the child cloned the root's target and the client predicts it
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<PredictionTarget>(server_child),
        stepper
            .server_app
            .world()
            .get::<PredictionTarget>(server_root),
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(client_child)
            .is_some(),
        "targeted client should predict the child"
    );

    // switch the root target away from the client, in place
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_root)
        .insert(PredictionTarget::to_clients(
            NetworkTarget::AllExceptSingle(client_id),
        ));
    stepper.frame_step_server_first(6);

    // the child followed the switch server-side...
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<PredictionTarget>(server_child),
        stepper
            .server_app
            .world()
            .get::<PredictionTarget>(server_root),
    );
    // ...and the client stopped predicting it
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(client_child)
            .is_none(),
        "untargeted client should no longer predict the child"
    );
}
