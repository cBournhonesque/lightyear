use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;

pub(crate) mod ordered_reliable;
mod sequenced_reliable;
mod unordered_reliable;
mod unordered_unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
pub trait ChannelReceiver: Send + Sync {
    // TODO: need to revisit this API.
    //  we shouldn't have to specify message/message_id ?

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: Message, message_id: MessageId);

    /// Reads a message from the internal buffer to get its content
    fn read_message(&mut self) -> Option<Message>;
}
