//! More advanced replication tests

use crate::protocol::{CompA, CompS, CompSimple};
use crate::stepper::*;
use bevy::prelude::{Timer, TimerMode};
use lightyear::prelude::*;
use lightyear_replication::message::UpdatesChannel;
use lightyear_transport::channel::ChannelKind;
use tracing::info;

/// Test that ReplicationMode::SinceLastAck is respected
/// - we keep sending replication packets until we receive an Ack
#[test]
fn test_since_last_ack() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0)))
        .id();
    let group_id = ReplicationGroupId(client_entity.to_bits());

    let tick_duration = stepper.tick_duration;
    stepper.advance_time(tick_duration);

    // send once to the server
    stepper.frame_step(1);

    // check that we sent an EntityActions message. (the ack tick gets updated immediately because we know the message will get acked)
    let actions_sent = stepper
        .client(0)
        .get::<ReplicationSender>()
        .unwrap()
        .group_channels
        .get(&group_id)
        .unwrap()
        .actions_next_send_message_id
        .0;
    assert_eq!(actions_sent, 1);

    stepper
        .client_app()
        .world_mut()
        .get_mut::<CompA>(client_entity)
        .unwrap()
        .0 = 2.0;

    // first update: we send an update to the server
    info!("first update");
    stepper.client_app().update();
    stepper.advance_time(tick_duration);

    // check that we send again to the server since we haven't received an ack
    info!("second update");
    stepper.client_app().update();

    // check that we re-sent an update since we didn't receive any ack.
    // (this time it's sent as an update, since the replication system already sent an EntityActions message.
    //  we only want to send an Insert when the component is first added)
    assert_eq!(
        stepper
            .client(0)
            .get::<Transport>()
            .unwrap()
            .senders
            .get(&ChannelKind::of::<UpdatesChannel>())
            .unwrap()
            .messages_sent
            .len(),
        1
    );

    // server receives the message and sends back an ack
    stepper.server_app.update();

    stepper.frame_step(1);

    // check that this time we don't send any replication message since our last message has been acked.
    let actions_sent = stepper
        .client(0)
        .get::<ReplicationSender>()
        .unwrap()
        .group_channels
        .get(&group_id)
        .unwrap()
        .actions_next_send_message_id
        .0;
    assert_eq!(actions_sent, 1);
    assert_eq!(
        stepper
            .client(0)
            .get::<Transport>()
            .unwrap()
            .senders
            .get(&ChannelKind::of::<UpdatesChannel>())
            .unwrap()
            .messages_sent
            .len(),
        0
    );

    // check that we have received an ack
    let group_channel = stepper
        .client(0)
        .get::<ReplicationSender>()
        .unwrap()
        .group_channels
        .get(&group_id)
        .unwrap();
    assert_ne!(group_channel.ack_bevy_tick, None);
}

/// Test that acks work correctly for updates split across multiple packets
///
/// Check that we don't get the log: "Received an update message-id ack but we don't know the corresponding group id"
#[test]
fn test_acks_multi_packet() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let str = "a".to_string();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompS(str)))
        .id();
    stepper.frame_step(3);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();

    let str = "X".repeat(2000);
    stepper
        .client_app()
        .world_mut()
        .get_mut::<CompS>(client_entity)
        .unwrap()
        .0 = str.clone();
    stepper.frame_step(3);

    // TODO: add a real assert here instead of just checking for the log
}

/// In the following situation:
/// - client 1 connects and server spawns and replicates a Predicted entity 1 to it
/// - client 2 connects and server spawns and replicates a Predicted entity 2 to it
///
/// When client 2 connects, we should NOT replicate entity 1 to client 1 again (even if they are in the same replication group)
/// because entity 1 was not updated. However we should replicate entity 2 to client 1.
///
/// We could not replicate entity 2 to client 1 because CachedReplicate for entity 2 could be updated before sender 1's replicate system runs.
#[test]
fn test_replicate_new_connection() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let server_sender = stepper.client_of(0).id();
    info!("ClientOf 0 is entity {:?}", server_sender);

    // give sender 0 a long SendTimer so that CachedReplicate could be updated before sender 0's replicate system runs
    stepper
        .client_of_mut(0)
        .get_mut::<ReplicationSender>()
        .unwrap()
        .send_timer = Timer::new(TICK_DURATION * 5, TimerMode::Repeating);

    let server_entity1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(PeerId::Netcode(0))),
            CompSimple(0.0),
        ))
        .id();
    stepper.frame_step(6);

    let client1_entity1 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity1)
        .unwrap();

    // new client connects
    stepper.new_client(ClientType::Netcode);
    stepper.init();

    info!("Spawning entity 2 on server");
    let server_entity2 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(PeerId::Netcode(1))),
            CompSimple(1.0),
        ))
        .id();
    stepper.frame_step(6);

    // check that client 1 received entity 2
    let client1_entity2 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity2)
        .unwrap();
    // check that client 1 did NOT receive entity 1 again
    assert_eq!(
        stepper.client_apps[0]
            .world()
            .get::<CompSimple>(client1_entity1)
            .unwrap()
            .0,
        0.0
    );
}
