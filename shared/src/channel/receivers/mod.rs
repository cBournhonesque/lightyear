use enum_dispatch::enum_dispatch;

use crate::packet::message::MessageContainer;

pub(crate) mod ordered_reliable;
pub(crate) mod sequenced_reliable;
pub(crate) mod sequenced_unreliable;
pub(crate) mod unordered_reliable;
pub(crate) mod unordered_unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelReceive<P> {
    // TODO: need to revisit this API.
    //  we shouldn't have to specify message/message_id ?

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: MessageContainer<P>) -> anyhow::Result<()>;

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<MessageContainer<P>>;
}

/// Enum dispatch lets us derive ChannelReceive on each enum variant
#[enum_dispatch(ChannelReceive<P>)]
pub enum ChannelReceiver<P> {
    UnorderedUnreliable(unordered_unreliable::UnorderedUnreliableReceiver<P>),
    SequencedUnreliable(sequenced_unreliable::SequencedUnreliableReceiver<P>),
    OrderedReliable(ordered_reliable::OrderedReliableReceiver<P>),
    SequencedReliable(sequenced_reliable::SequencedReliableReceiver<P>),
    UnorderedReliable(unordered_reliable::UnorderedReliableReceiver<P>),
}
