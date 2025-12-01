use crate::protocol::{CompA, CompMap};
use crate::stepper::*;
use lightyear_connection::prelude::*;
use lightyear_core::id::PeerId;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;
#[allow(unused_imports)]
use tracing::info;

#[test]
fn test_give_authority_server_to_client() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let sender = stepper.client_of(0).id();
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Server)
    );
    stepper.frame_step(2);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(Replicate::to_server());
    stepper.server_app.world_mut().trigger(GiveAuthority {
        entity: server_entity,
        remote_peer: Some(PeerId::Netcode(0)),
    });
    stepper.frame_step(2);

    // check that the server lost authority and client gained authority
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Netcode(0))
    );
    assert!(
        !stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert!(
        stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity)
    );

    // check that the server updates are not replicated
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompA(1.0));
    stepper.frame_step(2);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompA>(client_entity)
            .is_none()
    );

    // check that client updates are replicated
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(CompA(2.0));
    stepper.frame_step(2);
    assert_eq!(
        stepper.server_app.world().get::<CompA>(server_entity),
        Some(&CompA(2.0))
    );
}

#[test]
fn test_give_authority_client_to_server() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let sender = stepper.client_of(0).id();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn(Replicate::to_server())
        .id();
    stepper.frame_step(2);

    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Netcode(0))
    );
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(Replicate::to_clients(NetworkTarget::All));
    stepper.client_app().world_mut().trigger(GiveAuthority {
        entity: client_entity,
        remote_peer: Some(PeerId::Server),
    });
    stepper.frame_step(2);

    // check that the server lost authority and client gained authority
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Server)
    );
    assert!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert!(
        !stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity)
    );

    // check that the client updates are not replicated
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(CompA(1.0));
    stepper.frame_step(2);
    assert!(
        stepper
            .server_app
            .world()
            .get::<CompA>(server_entity)
            .is_none()
    );

    // check that server updates are replicated
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompA(2.0));
    stepper.frame_step(2);
    assert_eq!(
        stepper.client_app().world().get::<CompA>(client_entity),
        Some(&CompA(2.0))
    );
}

/// Spawn on client, transfer authority to server, despawn entity on server.
/// The entity should get despawned correctly on client.
/// Relevant issue: https://github.com/cBournhonesque/lightyear/issues/644
#[test]
fn test_transfer_authority_despawn() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let sender = stepper.client(0).id();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(Replicate::to_clients(NetworkTarget::All));

    stepper.client_app().world_mut().trigger(GiveAuthority {
        entity: client_entity,
        remote_peer: Some(PeerId::Server),
    });
    stepper.frame_step(2);

    // check that the client lost authority and server gained authority
    assert!(
        !stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity)
    );
    assert!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );

    // server despawn the entity
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .despawn();
    stepper.frame_step(2);

    // check that the client entity is also despawned
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_entity)
            .is_err()
    );
}

/// Spawn on client, transfer authority to server
/// Update on server, the updates from the server use entity mapping on the send side.
/// (both for the Entity in Updates and for the content of the components in the Update)
#[test]
fn test_transfer_authority_map_entities() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let sender = stepper.client(0).id();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            CompMap(server_entity),
        ));

    stepper.client_app().world_mut().trigger(GiveAuthority {
        entity: client_entity,
        remote_peer: Some(PeerId::Server),
    });
    stepper.frame_step(2);

    // check that the client lost authority and server gained authority
    assert!(
        !stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity)
    );
    assert!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );

    // check that server sent an update and that the components were mapped correctly
    assert_eq!(
        stepper.client_app().world().get::<CompMap>(client_entity),
        Some(&CompMap(client_entity))
    );
}

/// Spawn on client, transfer authority from client 1 to client 2
#[test]
fn test_transfer_authority_client_to_client() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_sender_0 = stepper.client(0).id();
    // send an entity from client 0 to server
    let client_entity_0 = stepper.client_apps[0]
        .world_mut()
        .spawn((Replicate::manual(vec![client_sender_0]),))
        .id();
    info!("Client entity 0: {client_entity_0:?}");
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity_0)
        .expect("entity is not present in entity map");
    assert!(
        stepper
            .client_of_mut(0)
            .get_mut::<ReplicationSender>()
            .unwrap()
            .replicated_entities
            .get(&server_entity)
            .is_some()
    );
    assert!(
        !stepper
            .client_of_mut(0)
            .get_mut::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Netcode(0))
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert((
            Replicate::to_clients(NetworkTarget::All),
            CompMap(server_entity),
        ));
    assert!(
        !stepper
            .client_of_mut(0)
            .get_mut::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert!(
        stepper
            .client_of_mut(1)
            .get_mut::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert_eq!(
        stepper
            .server()
            .get::<AuthorityBroker>()
            .unwrap()
            .owners
            .get(&server_entity)
            .unwrap(),
        &Some(PeerId::Netcode(0))
    );

    stepper.frame_step(2);

    // check that the client 1 has the replicated entity
    let client_entity_1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");
    // the authority can be freely stolen
    stepper.client_apps[1]
        .world_mut()
        .entity_mut(client_entity_1)
        .insert(AuthorityTransfer::Steal);

    // give the authority from client 0 to client 1
    stepper.client_apps[0].world_mut().trigger(GiveAuthority {
        entity: client_entity_0,
        remote_peer: Some(PeerId::Netcode(1)),
    });
    stepper.frame_step(4);

    // check that the client 0 lost authority and client 1 gained authority
    assert!(
        !stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity_0)
    );
    assert!(
        stepper
            .client(1)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity_1)
    );
    assert!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert!(
        !stepper
            .client_of(1)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );

    // request the authority from client 0 to client 1
    stepper.client_apps[0]
        .world_mut()
        .trigger(RequestAuthority {
            entity: client_entity_0,
        });
    stepper.frame_step(4);

    // check that the client 1 lost authority and client 0 gained authority
    assert!(
        stepper
            .client(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity_0)
    );
    assert!(
        !stepper
            .client(1)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(client_entity_1)
    );
    assert!(
        !stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
    assert!(
        stepper
            .client_of(1)
            .get::<ReplicationSender>()
            .unwrap()
            .has_authority(server_entity)
    );
}
