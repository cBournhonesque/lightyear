use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
use bytes::Bytes;
use crossbeam_channel::Receiver;
use enum_dispatch::enum_dispatch;
use std::collections::VecDeque;

pub(crate) mod fragment_ack_receiver;
pub(crate) mod fragment_sender;
pub(crate) mod reliable;
pub(crate) mod sequenced_unreliable;
pub(crate) mod tick_unreliable;
pub(crate) mod unordered_unreliable;
pub(crate) mod unordered_unreliable_with_acks;

// TODO: separate trait into multiple traits
// - buffer send should be public
// - all other methods should be private
/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelSend {
    /// Bookkeeping for the channel
    fn update(
        &mut self,
        time_manager: &TimeManager,
        ping_manager: &PingManager,
        tick_manager: &TickManager,
    );

    /// Queues a message to be transmitted.
    /// The priority of the message needs to be specified
    ///
    /// Returns the MessageId of the message that was queued, if there is one
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId>;

    /// Reads from the buffer of messages to send to prepare a list of Packets
    /// that can be sent over the network for this channel
    fn send_packet(&mut self) -> (VecDeque<SingleData>, VecDeque<FragmentData>);

    /// Collect the list of messages that need to be sent
    /// Either because they have never been sent, or because they need to be resent (for reliability)
    /// Needs to be called before [`ReliableSender::send_packet`](reliable::ReliableSender::send_packet)
    fn collect_messages_to_send(&mut self);

    /// Called when we receive acknowledgement that a Message has been received
    fn notify_message_delivered(&mut self, message_ack: &MessageAck);

    /// Returns true if there are messages in the buffer that are ready to be sent
    fn has_messages_to_send(&self) -> bool;

    /// Create a new receiver that will receive a message id when a sent message is acked
    fn subscribe_acks(&mut self) -> Receiver<MessageId>;
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[enum_dispatch(ChannelSend)]
pub enum ChannelSender {
    UnorderedUnreliableWithAcks(unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender),
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableSender),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableSender),
    Reliable(reliable::ReliableSender),
    TickUnreliable(tick_unreliable::TickUnreliableSender),
}
