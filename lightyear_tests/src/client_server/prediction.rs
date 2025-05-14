use crate::protocol::CompFull;
use crate::stepper::ClientServerStepper;
use bevy::prelude::{ChildOf, Entity, With};
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::Predicted;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::components::{Confirmed, PrePredicted};
use lightyear_replication::prelude::Replicate;
use test_log::test;

/// Simple PrePrediction case
#[test]
fn test_pre_prediction() {

    let mut stepper = ClientServerStepper::single();

    // spawn a pre-predicted entity on the client
    let predicted_entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            Replicate::to_server(),
            CompFull(1.0),
            PrePredicted::default(),
        ))
        .id();

    // flush to apply pre-predicted related commands
    stepper.flush();

    // check that the confirmed entity was spawned
    let confirmed_entity = stepper
        .client_app()
        .world_mut()
        .query_filtered::<Entity, With<Confirmed>>()
        .single(stepper.client_app().world())
        .unwrap();

    // need to step multiple times because the server entity doesn't handle messages from future ticks
    stepper.frame_step(10);

    // check that the server has received the entity
    // (we map from confirmed to server entity because the server updates its entity-mapping
    // upon reception of a pre-predicted entity)
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(confirmed_entity)
        .expect("entity is not present in entity map");

    // check that the server's updates are replicated properly
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<CompFull>(server_entity)
            .unwrap()
            .0,
        1.0
    );

    // insert Replicate on the server entity
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            CompFull(2.0)
        ));

    stepper.frame_step(2);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(confirmed_entity)
            .unwrap()
            .0,
        2.0
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(predicted_entity)
            .is_some()
    );
}


/// Test that PrePredicted works if ReplicateHierarchy is present.
/// In that case, both the parent but also the children should be pre-predicted.
///
/// The child-of relationship should be present on the pre-predicted entities
#[test]
fn test_pre_prediction_hierarchy() {
    let mut stepper = ClientServerStepper::single();
    let child = stepper
        .client_app()
        .world_mut()
        .spawn(CompFull(0.0))
        .id();
    let parent = stepper
        .client_app()
        .world_mut()
        .spawn((
            Replicate::to_server(),
            PrePredicted::default(),
        ))
        .add_child(child)
        .id();
    stepper.frame_step(1);

    let confirmed_parent = stepper
        .client_app()
        .world_mut()
        .get::<PrePredicted>(parent)
        .unwrap()
        .confirmed_entity
        .unwrap();
    assert!(stepper
        .client_app()
        .world()
        .get::<Confirmed>(confirmed_parent)
        .is_some());
    // check that PrePredicted was also added on the child
    let confirmed_child = stepper
        .client_app()
        .world_mut()
        .get::<PrePredicted>(child)
        .unwrap()
        .confirmed_entity
        .unwrap();
    assert!(stepper
        .client_app()
        .world()
        .get::<Confirmed>(confirmed_child)
        .is_some());
    
    // check that both the parent and the child were replicated
    let server_parent = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(confirmed_parent)
        .expect("entity is not present in entity map");
    let server_child = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(confirmed_child)
        .expect("entity is not present in entity map");

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ChildOf>(server_child)
            .unwrap()
            .parent(),
        server_parent
    );

    // add Replicate on the server side
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_parent)
        .insert(Replicate::to_clients(NetworkTarget::All));

    stepper.frame_step(2);

    // check that the client parent and child entity both have the Predicted component
    assert_eq!(stepper
        .client_app()
        .world()
        .get::<Predicted>(parent)
        .unwrap().confirmed_entity, Some(confirmed_parent));
    assert_eq!(stepper
        .client_app()
        .world()
        .get::<Predicted>(child)
        .unwrap().confirmed_entity, Some(confirmed_child));
}