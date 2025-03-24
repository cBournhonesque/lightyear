use alloc::collections::VecDeque;

use bytes::Bytes;
use crossbeam_channel::Receiver;
use enum_dispatch::enum_dispatch;

use crate::packet::message::{MessageAck, MessageId, SendMessage};
use crate::serialize::SerializationError;
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

pub(crate) mod fragment_ack_receiver;
pub(crate) mod fragment_sender;
pub(crate) mod reliable;
pub(crate) mod sequenced_unreliable;
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
    fn buffer_send(
        &mut self,
        message: Bytes,
        priority: f32,
    ) -> Result<Option<MessageId>, SerializationError>;

    /// Reads from the buffer of messages to send to prepare a list of Packets
    /// that can be sent over the network for this channel
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>);

    /// Called when we receive acknowledgement that a Message has been received
    fn receive_ack(&mut self, message_ack: &MessageAck);

    /// Create a new receiver that will receive a message id when a sent message is acked
    fn subscribe_acks(&mut self) -> Receiver<MessageId>;

    /// Create a new receiver that will receive a message id when a sent message on this channel
    /// has been lost by the remote peer
    fn subscribe_nacks(&mut self) -> Receiver<MessageId>;

    /// Send nacks to the subscribers of nacks
    fn send_nacks(&mut self, nack: MessageId);
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[derive(Debug)]
#[enum_dispatch(ChannelSend)]
pub enum ChannelSender {
    UnorderedUnreliableWithAcks(unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender),
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableSender),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableSender),
    Reliable(reliable::ReliableSender),
}
