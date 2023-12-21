use crate::packet::message::MessageContainer;
use crate::packet::message::{FragmentData, SingleData};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;
use enum_dispatch::enum_dispatch;

/// Utilities to receive a Message from multiple fragment packets
pub(crate) mod fragment_receiver;

/// Receive messages in an Ordered Reliable manner
pub(crate) mod ordered_reliable;

/// Receive messages in an Sequenced Reliable manner
pub(crate) mod sequenced_reliable;

/// Receive messages in an Sequenced Unreliable manner
pub(crate) mod sequenced_unreliable;

pub(crate) mod tick_unreliable;

/// Receive messages in an Unordered Reliable manner
pub(crate) mod unordered_reliable;

/// Receive messages in an Unordered Unreliable manner
pub(crate) mod unordered_unreliable;

/// A trait for receiving messages over a channel
#[enum_dispatch]
pub trait ChannelReceive {
    /// Bookkeeping on the channel
    fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager);

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()>;

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<SingleData>;
}

/// This enum contains the various types of receivers available
#[enum_dispatch(ChannelReceive)]
pub enum ChannelReceiver {
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableReceiver),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableReceiver),
    OrderedReliable(ordered_reliable::OrderedReliableReceiver),
    SequencedReliable(sequenced_reliable::SequencedReliableReceiver),
    UnorderedReliable(unordered_reliable::UnorderedReliableReceiver),
    TickUnreliable(tick_unreliable::TickUnreliableReceiver),
}
