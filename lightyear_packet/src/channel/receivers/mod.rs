//! This module contains the various types of receivers available to receive messages over a channel
use bytes::Bytes;
use enum_dispatch::enum_dispatch;

use crate::packet::message::ReceiveMessage;
use error::Result;
use lightyear_core::tick::Tick;
use lightyear_core::time::WrappedTime;

/// Utilities to receive a Message from multiple fragment packets
pub(crate) mod fragment_receiver;

/// Receive messages in an Ordered Reliable manner
pub(crate) mod ordered_reliable;

/// Receive messages in an Sequenced Reliable manner
pub(crate) mod sequenced_reliable;

/// Receive messages in an Sequenced Unreliable manner
pub(crate) mod sequenced_unreliable;

/// Receive messages in an Unordered Reliable manner
pub(crate) mod unordered_reliable;

pub(crate) mod error;
/// Receive messages in an Unordered Unreliable manner
pub(crate) mod unordered_unreliable;

/// A trait for receiving messages over a channel
#[enum_dispatch]
pub trait ChannelReceive {
    /// Bookkeeping on the channel
    fn update(&mut self, now: WrappedTime);

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<()>;

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<(Tick, Bytes)>;
}

/// This enum contains the various types of receivers available
#[derive(Debug)]
#[enum_dispatch(ChannelReceive)]
pub enum ChannelReceiverEnum {
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableReceiver),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableReceiver),
    OrderedReliable(ordered_reliable::OrderedReliableReceiver),
    SequencedReliable(sequenced_reliable::SequencedReliableReceiver),
    UnorderedReliable(unordered_reliable::UnorderedReliableReceiver),
}
