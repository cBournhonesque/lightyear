use crate::channel::receivers::ChannelReceive;
use anyhow::anyhow;
use std::collections::VecDeque;

use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;

/// Sequenced Unreliable receiver:
/// do not return messages in order, but ignore the messages that are older than the most recent one received
pub struct SequencedUnreliableReceiver {
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: VecDeque<Message>,
    /// Highest message id received so far
    most_recent_message_id: MessageId,
}

impl SequencedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
            most_recent_message_id: MessageId(0),
        }
    }
}

impl ChannelReceive for SequencedUnreliableReceiver {
    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: Message) -> anyhow::Result<()> {
        let message_id = message.id.ok_or_else(|| anyhow!("message id not found"))?;

        // if the message is too old, ignore it
        if message_id < self.most_recent_message_id {
            return Ok(());
        }

        // update the most recent message id
        if message_id > self.most_recent_message_id {
            self.most_recent_message_id = message_id;
        }

        // add the message to the buffer
        self.recv_message_buffer.push_back(message);
        Ok(())
    }
    fn read_message(&mut self) -> Option<Message> {
        self.recv_message_buffer.pop_front()
        // TODO: naia does a more optimized version by return a Vec<Message> instead of Option<Message>
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelReceive;
    use super::SequencedUnreliableReceiver;
    use super::{Message, MessageId};
    use bytes::Bytes;

    #[test]
    fn test_ordered_reliable_receiver_internals() {
        let mut receiver = SequencedUnreliableReceiver::new();

        let mut message1 = Message::new(Bytes::from("hello"));
        let mut message2 = Message::new(Bytes::from("world"));
        let mut message3 = Message::new(Bytes::from("test"));

        // receive an old message: it doesn't get added to the buffer
        message2.id = Some(MessageId(60000));
        receiver.buffer_recv(message2.clone());
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        message2.id = Some(MessageId(1));
        receiver.buffer_recv(message2.clone());

        // the message has been buffered, and we can read it instantly
        // since it's the most recent
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.most_recent_message_id, MessageId(1));
        assert_eq!(receiver.read_message(), Some(message2.clone()));
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive an earlier message 0
        message1.id = Some(MessageId(0));
        receiver.buffer_recv(message1.clone());
        // we don't add it to the buffer since we have read a more recent message.
        assert_eq!(receiver.recv_message_buffer.len(), 0);
        assert_eq!(receiver.read_message(), None);

        // receive a later message
        message3.id = Some(MessageId(2));
        receiver.buffer_recv(message3.clone());
        // it's the most recent message so we receive it
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.most_recent_message_id, MessageId(2));
        assert_eq!(receiver.read_message(), Some(message3.clone()));
        assert_eq!(receiver.recv_message_buffer.len(), 0);
    }
}
