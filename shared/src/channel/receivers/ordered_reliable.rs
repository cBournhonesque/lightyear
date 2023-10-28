use std::collections::{btree_map, BTreeMap};

use anyhow::anyhow;
use bytes::Bytes;

use crate::channel::receivers::ChannelReceive;
use crate::packet::message::MessageContainer;
use crate::packet::packet::FragmentData;
use crate::packet::wrapping_id::MessageId;

/// Ordered Reliable receiver: make sure that all messages are received,
/// and return them in order
pub struct OrderedReliableReceiver<P> {
    /// Next message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids sequentially.
    pending_recv_message_id: MessageId,
    // TODO: optimize via ring buffer?
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, MessageContainer<P>>,
}

impl<P> OrderedReliableReceiver<P> {
    pub fn new() -> Self {
        Self {
            pending_recv_message_id: MessageId(0),
            recv_message_buffer: BTreeMap::new(),
        }
    }
}

impl<P> ChannelReceive<P> for OrderedReliableReceiver<P> {
    fn buffer_recv_fragment(&mut self, fragment: FragmentData) -> anyhow::Result<()> {
        todo!()
    }

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: MessageContainer<P>) -> anyhow::Result<()> {
        let message_id = message.id.ok_or_else(|| anyhow!("message id not found"))?;

        // if the message is too old, ignore it
        if message_id < self.pending_recv_message_id {
            return Ok(());
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            entry.insert(message);
        }
        Ok(())
    }

    /// Reads a message from the internal buffer to get its content
    /// Since we are receiving messages in order, we don't return from the buffer
    /// until we have received the message we are waiting for (the next expected MessageId)
    /// This assumes that the sender sends all message ids sequentially.
    fn read_message(&mut self) -> Option<MessageContainer<P>> {
        // Check if we have received the message we are waiting for
        let Some(message) = self
            .recv_message_buffer
            .remove(&self.pending_recv_message_id)
        else {
            return None;
        };

        // if we have finally received the message we are waiting for, return it and
        // wait for the next one
        self.pending_recv_message_id += 1;
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
    use crate::channel::receivers::ChannelReceive;
    use crate::packet::wrapping_id::MessageId;
    use crate::MessageContainer;

    #[test]
    fn test_ordered_reliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = OrderedReliableReceiver::<i32>::new();

        let mut message1 = MessageContainer::new(0);
        let mut message2 = MessageContainer::new(1);

        // receive an old message: it doesn't get added to the buffer because the next one we expect is 0
        message2.id = Some(MessageId(60000));
        receiver.buffer_recv(message2.clone())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        message2.id = Some(MessageId(1));
        receiver.buffer_recv(message2.clone())?;

        // the message has been buffered, but we are not processing it yet
        // until we have received message 0
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.get(&MessageId(1)).is_some());
        assert_eq!(receiver.read_message(), None);
        assert_eq!(receiver.pending_recv_message_id, MessageId(0));

        // receive message 0
        message1.id = Some(MessageId(0));
        receiver.buffer_recv(message1.clone())?;
        assert_eq!(receiver.recv_message_buffer.len(), 2);

        // now we can read the messages in order
        assert_eq!(receiver.read_message(), Some(message1.clone()));
        assert_eq!(receiver.pending_recv_message_id, MessageId(1));
        assert_eq!(receiver.read_message(), Some(message2.clone()));
        Ok(())
    }
}
