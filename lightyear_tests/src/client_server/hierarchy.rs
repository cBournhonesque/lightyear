//! Check various replication scenarios between 2 peers only

use crate::stepper::*;
use bevy::prelude::*;
use lightyear::prelude::*;
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
