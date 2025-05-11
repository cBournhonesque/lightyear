use crate::protocol::CompA;
use crate::stepper::ClientServerStepper;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::id::PeerId;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;

#[test]
fn test_give_authority() {
    let mut stepper = ClientServerStepper::single();

    let server_entity = stepper.server_app.world_mut().spawn((
        Replicate::to_clients(NetworkTarget::All),
    )).id();
    stepper.frame_step(2);
    let client_entity = stepper.client(0).get::<MessageManager>().unwrap().entity_mapper.get_local(server_entity)
        .expect("entity is not present in entity map");
    stepper.client_app.world_mut().entity_mut(client_entity).insert(
        Replicate::to_server().without_authority()
    );
    stepper.server_app.world_mut().trigger(GiveAuthority {
        entity: server_entity,
        remote_peer: PeerId::Netcode(0)
    });
    stepper.frame_step(2);

    // check that the server lost authority and client gained authority
    assert!(!stepper.client_of(0).get::<ReplicationSender>().unwrap().has_authority(server_entity));
    assert!(stepper.client(0).get::<ReplicationSender>().unwrap().has_authority(client_entity));

    // check that the server updates are not replicated
    stepper.server_app.world_mut().entity_mut(server_entity).insert(CompA(1.0));
    stepper.frame_step(2);
    assert!(stepper.client_app.world().get::<CompA>(client_entity).is_none());

    // check that client updates are replicated
    stepper.client_app.world_mut().entity_mut(client_entity).insert(CompA(2.0));
    stepper.frame_step(2);
    assert_eq!(stepper.server_app.world().get::<CompA>(server_entity), Some(&CompA(2.0)));
}