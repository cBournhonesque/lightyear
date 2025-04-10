//! Check various replication scenarios between 2 peers only

use crate::protocol::CompA;
use crate::stepper::ClientServerStepper;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::Replicate;
use test_log::test;

#[test]
fn test_spawn() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
    )).id();
    // TODO: might need to step more when syncing to avoid receiving updates from the past?
    stepper.frame_step(1);
    stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");
}

#[test]
fn test_entity_despawn() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
    )).id();
    stepper.frame_step(1);
     let server_entity = stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");

    // despawn
    stepper.client_app.world_mut().despawn(client_entity);
    stepper.frame_step(1);

    // check that the entity was despawned
    assert!(stepper
        .server_app
        .world()
        .get_entity(server_entity)
        .is_err());
}

#[test]
fn test_despawn_from_replicate_change() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
    )).id();
    stepper.frame_step(1);
     let server_entity = stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");

    // update replicate to exclude the previous sender
    stepper.client_app.world_mut().entity_mut(client_entity).insert(Replicate::manual(vec![]));
    stepper.frame_step(1);

    // check that the entity was despawned on the previous sender
    assert!(stepper
        .server_app
        .world()
        .get_entity(server_entity)
        .is_err());
}

#[test]
fn test_spawn_from_replicate_change() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::manual(vec![]),
    )).id();
    stepper.frame_step(1);
     assert!(stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).is_none());

    // update replicate to include a new sender
    stepper.client_app.world_mut().entity_mut(client_entity).insert(Replicate::to_server());
    stepper.frame_step(1);

    let server_entity = stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity)
        .expect("entity is not present in entity map");
}

#[test]
fn test_component_insert() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
    )).id();
    stepper.frame_step(1);
    let server_entity = stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).unwrap();

    stepper.client_app.world_mut().entity_mut(client_entity).insert(CompA(1.0));
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(1.0)
    );
}

#[test]
fn test_component_update() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicate::to_server(),
        CompA(1.0),
    )).id();
    stepper.frame_step(1);
    let server_entity = stepper.client_1().get::<MessageManager>().unwrap().entity_mapper.get_local(client_entity).unwrap();
     assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(1.0)
    );

    stepper.client_app.world_mut().entity_mut(client_entity).get_mut::<CompA>().unwrap().0 = 2.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(2.0)
    );
}
