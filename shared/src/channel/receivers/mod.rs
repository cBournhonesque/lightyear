use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;
use enum_dispatch::enum_dispatch;

pub(crate) mod ordered_reliable;
pub(crate) mod sequenced_reliable;
pub(crate) mod sequenced_unreliable;
pub(crate) mod unordered_reliable;
pub(crate) mod unordered_unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelReceive: Send + Sync {
    // TODO: need to revisit this API.
    //  we shouldn't have to specify message/message_id ?

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: Message) -> anyhow::Result<()>;

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<Message>;
}

/// Enum dispatch lets us derive ChannelReceive on each enum variant
#[enum_dispatch(ChannelReceive)]
pub enum ChannelReceiver {
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableReceiver),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableReceiver),
    OrderedReliable(ordered_reliable::OrderedReliableReceiver),
    SequencedReliable(sequenced_reliable::SequencedReliableReceiver),
    UnorderedReliable(unordered_reliable::UnorderedReliableReceiver),
}
