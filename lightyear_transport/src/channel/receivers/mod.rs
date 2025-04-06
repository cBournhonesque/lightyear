//! This module contains the various types of receivers available to receive messages over a channel
use bytes::Bytes;
use enum_dispatch::enum_dispatch;

use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
use crate::channel::receivers::sequenced_reliable::SequencedReliableReceiver;
use crate::channel::receivers::sequenced_unreliable::SequencedUnreliableReceiver;
use crate::channel::receivers::unordered_reliable::UnorderedReliableReceiver;
use crate::channel::receivers::unordered_unreliable::UnorderedUnreliableReceiver;
use crate::packet::message::{MessageId, ReceiveMessage};
use crate::prelude::{ChannelMode, ChannelSettings};
use core::time::Duration;
use error::Result;
use lightyear_core::tick::Tick;

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
    fn update(&mut self, now: Duration);

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<()>;

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<(Tick, Bytes, Option<MessageId>)>;
}

/// This enum contains the various types of receivers available
#[derive(Debug)]
#[enum_dispatch(ChannelReceive)]
pub enum ChannelReceiverEnum {
    UnorderedUnreliable(UnorderedUnreliableReceiver),
    SequencedUnreliable(SequencedUnreliableReceiver),
    OrderedReliable(OrderedReliableReceiver),
    SequencedReliable(SequencedReliableReceiver),
    UnorderedReliable(UnorderedReliableReceiver),
}


impl From<&ChannelSettings> for ChannelReceiverEnum {
    fn from(settings: &ChannelSettings) -> Self {
        match settings.mode {
            ChannelMode::UnorderedUnreliableWithAcks => {
                UnorderedUnreliableReceiver::new().into()
            }
            ChannelMode::UnorderedUnreliable => {
                UnorderedUnreliableReceiver::new().into()
            }
            ChannelMode::SequencedUnreliable => {
                SequencedUnreliableReceiver::new().into()
            }
            ChannelMode::UnorderedReliable(_) => {
                UnorderedReliableReceiver::new().into()
            }
            ChannelMode::SequencedReliable(_) => {
                SequencedReliableReceiver::new().into()
            }
            ChannelMode::OrderedReliable(_) => {
                OrderedReliableReceiver::new().into()
            }
        }
    }
}