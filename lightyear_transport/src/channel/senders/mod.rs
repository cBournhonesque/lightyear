use crate::channel::senders::reliable::ReliableSender;
use crate::channel::senders::sequenced_unreliable::SequencedUnreliableSender;
use crate::channel::senders::unordered_unreliable::UnorderedUnreliableSender;
use crate::channel::senders::unordered_unreliable_with_acks::UnorderedUnreliableWithAcksSender;
use crate::packet::message::{MessageAck, MessageId, SendMessage};
use crate::prelude::{ChannelMode, ChannelSettings};
use alloc::collections::VecDeque;
use bevy::prelude::{Real, Time};
use bytes::Bytes;
use enum_dispatch::enum_dispatch;
use lightyear_link::LinkStats;

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
    fn update(&mut self, real_time: &Time<Real>, link_stats: &LinkStats);

    /// Queues a message to be transmitted.
    /// The priority of the message needs to be specified
    ///
    /// Returns the MessageId of the message that was queued, if there is one
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId>;

    /// Reads from the buffer of messages to send to prepare a list of Packets
    /// that can be sent over the network for this channel
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>);

    /// Called when we receive acknowledgement that a Message has been received
    fn receive_ack(&mut self, message_ack: &MessageAck);
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[derive(Debug)]
#[enum_dispatch(ChannelSend)]
pub enum ChannelSenderEnum {
    UnorderedUnreliableWithAcks(UnorderedUnreliableWithAcksSender),
    UnorderedUnreliable(UnorderedUnreliableSender),
    SequencedUnreliable(SequencedUnreliableSender),
    Reliable(ReliableSender),
}

impl From<&ChannelSettings> for ChannelSenderEnum {
    fn from(settings: &ChannelSettings) -> Self {
        match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks => {
                UnorderedUnreliableWithAcksSender::new(settings.send_frequency).into()
            }
            ChannelMode::UnorderedUnreliable => {
                UnorderedUnreliableSender::new(settings.send_frequency).into()
            }
            ChannelMode::SequencedUnreliable => {
                SequencedUnreliableSender::new(settings.send_frequency).into()
            }
            ChannelMode::UnorderedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
            ChannelMode::SequencedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
            ChannelMode::OrderedReliable(reliable_settings) => {
                ReliableSender::new(reliable_settings, settings.send_frequency).into()
            }
        }
    }
}
