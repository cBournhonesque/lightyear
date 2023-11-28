use std::collections::VecDeque;

use bytes::Bytes;
use enum_dispatch::enum_dispatch;

use crate::packet::message::{FragmentData, MessageAck, SingleData};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

pub(crate) mod fragment_sender;
pub(crate) mod reliable;
pub(crate) mod sequenced_unreliable;
pub(crate) mod tick_unreliable;
pub(crate) mod unordered_unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelSend {
    /// Bookkeeping for the channel
    fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager);

    /// Queues a message to be transmitted
    fn buffer_send(&mut self, message: Bytes);

    /// Reads from the buffer of messages to send to prepare a list of Packets
    /// that can be sent over the network for this channel
    fn send_packet(&mut self) -> (VecDeque<SingleData>, VecDeque<FragmentData>);

    /// Collect the list of messages that need to be sent
    /// Either because they have never been sent, or because they need to be resent (for reliability)
    /// Needs to be called before [`ReliableSender::send_packet`]
    fn collect_messages_to_send(&mut self);

    /// Called when we receive acknowledgement that a Message has been received
    fn notify_message_delivered(&mut self, message_ack: &MessageAck);

    /// Returns true if there are messages in the buffer that are ready to be sent
    fn has_messages_to_send(&self) -> bool;
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[enum_dispatch(ChannelSend)]
pub enum ChannelSender {
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableSender),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableSender),
    Reliable(reliable::ReliableSender),
    TickUnreliable(tick_unreliable::TickUnreliableSender),
}
