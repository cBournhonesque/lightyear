//! More advanced replication tests

use crate::protocol::CompA;
use crate::stepper::ClientServerStepper;
use lightyear_replication::message::UpdatesChannel;
use lightyear_replication::prelude::{Replicate, ReplicationGroupId, ReplicationSender};
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::prelude::Transport;

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
    stepper.client_app().update();

    // check that we sent an EntityActions message
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

    stepper.advance_time(tick_duration);

    // check that we send again to the server since we haven't received an ack
    stepper.client_app().update();

    // check that we re-sent an update since we didn't receive any ack.
    // (this time it's sent as an update, since the replication system already sent an EntityActions message)
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
    //
}
