//! Check various replication scenarios between 2 peers only

use crate::stepper::ClientServerStepper;
use bevy::prelude::*;
use lightyear_messages::MessageManager;
use lightyear_replication::components::DisableReplicateHierarchy;
use lightyear_replication::prelude::{ChildOfSync, Replicate, ReplicateLike};
use test_log::test;

// TODO:
// - remove Replicate from a parent: child should get despawned
// -

/// Add a child to a replicated Entity: the child should be replicated
#[test]
fn test_spawn_with_child() {
    let mut stepper = ClientServerStepper::single();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
    )).id();
    stepper.frame_step(1);
    stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");

    let client_child = stepper.client_app.world_mut().spawn((
        ChildOf(client_entity),
    )).id();
    stepper.frame_step(1);
    stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_child)
        .expect("entity is not present in entity map");
}

///
#[test]
fn test_despawn_with_child() {

}

fn setup_hierarchy() -> (ClientServerStepper, Entity, Entity, Entity) {
    let mut stepper = ClientServerStepper::single();
     let grandparent = stepper
        .server_app
        .world_mut()
        .spawn_empty()
        .id();
    let parent = stepper
        .server_app
        .world_mut()
        .spawn(ChildOf(grandparent))
        .id();
    let child = stepper
        .server_app
        .world_mut()
        .spawn(ChildOf(parent))
        .id();
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
    let client_grandparent = stepper.client(0).get::<MessageManager>().unwrap().entity_mapper.get_local(grandparent)
        .expect("entity is not present in entity map");
    let client_parent = stepper.client(0).get::<MessageManager>().unwrap().entity_mapper.get_local(parent)
        .expect("entity is not present in entity map");

    let (client_parent, client_parent_sync, client_parent_component) = stepper
        .client_app
        .world_mut()
        .query::<(Entity, &ChildOfSync, &ChildOf)>()
        .single(stepper.client_app.world())
        .unwrap();

    assert_eq!(client_parent_sync.entity, Some(client_grandparent));
    assert_eq!(client_parent_component.get(), client_grandparent);

    todo!("check that the parent/grandparent have the same ReplicationGroupId");


    // check that the child did not get replicated
    assert!(stepper
        .server_app
        .world()
        .get::<ChildOfSync>(child)
        .is_none());
    assert!(stepper
        .server_app
        .world()
        .get::<ReplicateLike>(child)
        .is_none());

    // remove the hierarchy on the sender side
    stepper
        .server_app
        .world_mut()
        .entity_mut(parent)
        .remove::<ChildOf>();
    let replicate_like = stepper.server_app.world_mut().get::<ReplicateLike>(parent);
    stepper.frame_step(2);

    // 1. make sure that parent sync has been updated on the sender side
    assert_eq!(
        stepper
            .server_app
            .world_mut()
            .entity_mut(parent)
            .get::<ChildOfSync>(),
        Some(&ChildOfSync::from(None))
    );

    // 2. make sure that the parent has been removed on the receiver side, and that ParentSync has been updated
    assert_eq!(
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_parent)
            .get::<ChildOfSync>(),
        Some(&ChildOfSync::from(None))
    );
    assert_eq!(
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_parent)
            .get::<ChildOf>(),
        None,
    );
    assert!(stepper
        .client_app
        .world_mut()
        .entity_mut(client_grandparent)
        .get::<Children>()
        .is_none());
}


/// https://github.com/cBournhonesque/lightyear/issues/649
/// P1 with child C1
/// If you add a new client to the replication target of P1, then both
/// P1 and C1 should be replicated to the new client.
/// (the issue says that only P1 was replicated)
#[test]
fn test_new_client_is_added_to_parent() {

}

/// https://github.com/cBournhonesque/lightyear/issues/547
/// Test that when a new child is added to a parent
/// the child is also replicated to the remote
#[test]
fn test_propagate_hierarchy_new_child() {
}

