use std::collections::{btree_map, BTreeMap};

use anyhow::anyhow;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, MessageId, SingleData};
pub use crate::shared::tick_manager::TickManager;
pub use crate::shared::time_manager::TimeManager;

/// Ordered Reliable receiver: make sure that all messages are received,
/// and return them in order
pub struct OrderedReliableReceiver {
    /// Next message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids sequentially.
    pending_recv_message_id: MessageId,
    // TODO: optimize via ring buffer?
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, SingleData>,
    fragment_receiver: FragmentReceiver,
}

impl OrderedReliableReceiver {
    pub fn new() -> Self {
        Self {
            pending_recv_message_id: MessageId(0),
            recv_message_buffer: BTreeMap::new(),
            fragment_receiver: FragmentReceiver::new(),
        }
    }
}

impl ChannelReceive for OrderedReliableReceiver {
    fn update(&mut self, _: &TimeManager, _: &TickManager) {}

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        let message_id = message
            .message_id()
            .ok_or_else(|| anyhow!("message id not found"))?;

        // if the message is too old, ignore it
        if message_id < self.pending_recv_message_id {
            return Ok(());
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            match message {
                MessageContainer::Single(data) => {
                    entry.insert(data);
                }
                MessageContainer::Fragment(data) => {
                    if let Some(single_data) =
                        self.fragment_receiver.receive_fragment(data, None)?
                    {
                        entry.insert(single_data);
                    }
                }
            }
        }
        Ok(())
    }

    /// Reads a message from the internal buffer to get its content
    /// Since we are receiving messages in order, we don't return from the buffer
    /// until we have received the message we are waiting for (the next expected MessageId)
    /// This assumes that the sender sends all message ids sequentially.
    fn read_message(&mut self) -> Option<SingleData> {
        // Check if we have received the message we are waiting for
        let message = self
            .recv_message_buffer
            .remove(&self.pending_recv_message_id)?;

        // if we have finally received the message we are waiting for, return it and
        // wait for the next one
        self.pending_recv_message_id += 1;
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::{MessageId, SingleData};

    #[test]
    fn test_ordered_reliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = OrderedReliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"), 1.0);
        let mut single2 = SingleData::new(None, Bytes::from("world"), 1.0);

        // receive an old message: it doesn't get added to the buffer because the next one we expect is 0
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(single2.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(single2.clone().into())?;

        // the message has been buffered, but we are not processing it yet
        // until we have received message 0
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.get(&MessageId(1)).is_some());
        assert_eq!(receiver.read_message(), None);
        assert_eq!(receiver.pending_recv_message_id, MessageId(0));

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(single1.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 2);

        // now we can read the messages in order
        assert_eq!(receiver.read_message(), Some(single1.clone()));
        assert_eq!(receiver.pending_recv_message_id, MessageId(1));
        assert_eq!(receiver.read_message(), Some(single2.clone()));
        Ok(())
    }
}
