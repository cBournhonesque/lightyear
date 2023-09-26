use crate::channel::receivers::ChannelReceive;
use anyhow::anyhow;

use std::collections::{btree_map, BTreeMap};

use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;

/// Unordered Reliable receiver: make sure that all messages are received,
/// and return them in any order
pub struct UnorderedReliableReceiver {
    /// Next oldest message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids.
    oldest_pending_message_id: MessageId,
    // TODO: optimize via ring buffer?
    // TODO: actually we could just use a VecDeque here?
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, Message>,
}

impl UnorderedReliableReceiver {
    pub fn new() -> Self {
        Self {
            oldest_pending_message_id: MessageId(0),
            recv_message_buffer: BTreeMap::new(),
        }
    }
}

impl ChannelReceive for UnorderedReliableReceiver {
    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: Message) -> anyhow::Result<()> {
        let message_id = message.id.ok_or_else(|| anyhow!("message id not found"))?;

        // we have already received the message if it's older than the oldest pending message
        // (since we are reliable, we should have received all messages prior to that one)
        if message_id < self.oldest_pending_message_id {
            return Ok(());
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            entry.insert(message);
        }
        Ok(())
    }
    fn read_message(&mut self) -> Option<Message> {
        // get the oldest received message
        let Some((message_id, message)) = self.recv_message_buffer.pop_first() else {
            return None;
        };

        if self.oldest_pending_message_id == message_id {
            self.oldest_pending_message_id += 1;
        }

        Some(message)
    }
}
