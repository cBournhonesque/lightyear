//! Check various replication scenarios between 2 peers only

use crate::stepper::ClientServerStepper;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{NetworkVisibility, Replicate, ReplicationGroup};
use test_log::test;

#[test]
fn test_spawn_gain_visibility() {
    let mut stepper = ClientServerStepper::single();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
        NetworkVisibility::default(),
    )).id();
    // entity is not visible because NetworkVisibility doesn't include it
    stepper.frame_step(1);
    assert!(stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).is_none());

    // gain visibility
    stepper.client_app.world_mut().get_mut::<NetworkVisibility>(client_entity).unwrap().gain_visibility(stepper.client_entities[0]);
    stepper.frame_step(1);
    stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");

    // TODO: gain visibility again: a spawn message should not be sent
}

#[test]
fn test_despawn_lose_visibility() {
    let mut stepper = ClientServerStepper::single();

    let mut visibility = NetworkVisibility::default();
    visibility.gain_visibility(stepper.client_entities[0]);
    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
        visibility,
    )).id();
    // entity is visible because of NetworkVisibility
    stepper.frame_step(1);
    let server_entity = stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).unwrap();

    // lose visibility: a Despawn message should be sent
    stepper.client_app.world_mut().get_mut::<NetworkVisibility>(client_entity).unwrap().lose_visibility(stepper.client_entities[0]);
    stepper.frame_step(1);
    assert!(stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).is_none());
}

/// Test that if an entity with NetworkVisibility is despawned, the DespawnMessage
/// is only sent to entities that have visibility on it.
#[test]
fn test_despawn_with_visibility() {
    let mut stepper: ClientServerStepper<2> = ClientServerStepper::default();

    let mut visibility_0 = NetworkVisibility::default();
    visibility_0.gain_visibility(stepper.client_entities[0]);
    let client_entity_0 = stepper.client_app.world_mut().spawn((
        Replicate::manual(vec![stepper.client_entities[0], stepper.client_entities[1]]),
        visibility_0,
        ReplicationGroup::new_id(1),
    )).id();

    let mut visibility_1 = NetworkVisibility::default();
    visibility_1.gain_visibility(stepper.client_entities[1]);
    let client_entity_1 = stepper.client_app.world_mut().spawn((
        Replicate::manual(vec![stepper.client_entities[0], stepper.client_entities[1]]),
        visibility_1,
        ReplicationGroup::new_id(1),
    )).id();

    stepper.frame_step(1);
    let server_entity_0 = stepper.client_of(0).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity_0).unwrap();
    let server_entity_1 = stepper.client_of(1).get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity_1).unwrap();

    // update the entity_map on the second connection to re-use the same entity
    // as client 1, so that we can check if the second connection also receives a despawn
    // when we despawn the entity for the first connection.
    //
    // i.e. a DespawnMessage for client_entity_0 send to all clients will also despawn server_entity_1
    // This only works because the replication group is the same for both entities.
    stepper.client_of_mut(1).get_mut::<MessageManager>().unwrap().entity_mapper.insert(client_entity_0, server_entity_1);

    stepper.client_app.world_mut().despawn(client_entity_0);
    stepper.frame_step(1);

    // server entity 0 has been despawned
    assert!(stepper.server_app.world().get_entity(server_entity_0).is_err());

    // server entity 1 has not been despawned (because connection 1 did not have visibility on client_entity_0)
    assert!(stepper.server_app.world().get_entity(server_entity_1).is_ok());
}

/// Test that when we add NetworkVisibility, the entity is despawned on senders
/// that are not present in the NetworkVisibility?
#[test]
fn test_despawn_add_network_visibility() {
}