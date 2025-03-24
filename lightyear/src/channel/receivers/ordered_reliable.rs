use alloc::collections::{btree_map, BTreeMap};

use bytes::Bytes;

use super::error::{ChannelReceiveError, Result};
use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};
use crate::prelude::Tick;
pub use crate::shared::tick_manager::TickManager;
pub use crate::shared::time_manager::TimeManager;

/// Ordered Reliable receiver: make sure that all messages are received,
/// and return them in order
#[derive(Debug)]
pub struct OrderedReliableReceiver {
    /// Next message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids sequentially.
    pending_recv_message_id: MessageId,
    // TODO: optimize via ring buffer?
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, (Tick, Bytes)>,
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
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<()> {
        let message_id = message
            .data
            .message_id()
            .ok_or(ChannelReceiveError::MissingMessageId)?;

        // if the message is too old, ignore it
        if message_id < self.pending_recv_message_id {
            return Ok(());
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            match message.data {
                MessageData::Single(single) => {
                    entry.insert((message.remote_sent_tick, single.bytes));
                }
                MessageData::Fragment(fragment) => {
                    if let Some(res) = self.fragment_receiver.receive_fragment(
                        fragment,
                        message.remote_sent_tick,
                        None,
                    ) {
                        entry.insert(res);
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
    fn read_message(&mut self) -> Option<(Tick, Bytes)> {
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
    use crate::packet::message::{MessageId, ReceiveMessage, SingleData};
    use crate::prelude::{PacketError, Tick};

    #[test]
    fn test_ordered_reliable_receiver_internals() -> Result<(), PacketError> {
        let mut receiver = OrderedReliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"));
        let mut single2 = SingleData::new(None, Bytes::from("world"));

        // receive an old message: it doesn't get added to the buffer because the next one we expect is 0
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(ReceiveMessage {
            data: single2.clone().into(),
            remote_sent_tick: Tick(1),
        })?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(ReceiveMessage {
            data: single2.clone().into(),
            remote_sent_tick: Tick(2),
        })?;

        // the message has been buffered, but we are not processing it yet
        // until we have received message 0
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.contains_key(&MessageId(1)));
        assert_eq!(receiver.read_message(), None);
        assert_eq!(receiver.pending_recv_message_id, MessageId(0));

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(ReceiveMessage {
            data: single1.clone().into(),
            remote_sent_tick: Tick(3),
        })?;
        assert_eq!(receiver.recv_message_buffer.len(), 2);

        // now we can read the messages in order
        assert_eq!(
            receiver.read_message(),
            Some((Tick(3), single1.bytes.clone()))
        );
        assert_eq!(receiver.pending_recv_message_id, MessageId(1));
        assert_eq!(
            receiver.read_message(),
            Some((Tick(2), single2.bytes.clone()))
        );
        Ok(())
    }
}
