//! More advanced replication tests

use crate::protocol::{CompA, CompS};
use crate::stepper::ClientServerStepper;
use lightyear_messages::MessageManager;
use lightyear_replication::message::UpdatesChannel;
use lightyear_replication::prelude::{Replicate, ReplicationGroupId, ReplicationSender};
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::prelude::Transport;
use tracing::info;

/// Test that ReplicationMode::SinceLastAck is respected
/// - we keep sending replication packets until we receive an Ack
#[test]
fn test_since_last_ack() {
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();

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
