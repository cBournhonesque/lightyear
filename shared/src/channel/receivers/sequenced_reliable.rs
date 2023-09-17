use crate::channel::receivers::ChannelReceiver;
use bytes::Bytes;
use std::collections::{btree_map, BTreeMap, BTreeSet, VecDeque};

use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;

/// Sequenced Reliable receiver: make sure that all messages are received,
/// do not return them in order, but ignore the messages that are older than the most recent one received
pub struct SequencedReliableReceiver {
    // TODO: optimize via ring buffer?
    // TODO: actually do we even need a buffer? we might just need a buffer of 1
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, Message>,
    /// Highest message id received so far
    most_recent_message_id: MessageId,
}

impl ChannelReceiver for SequencedReliableReceiver {
    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: Message, message_id: MessageId) {
        // if the message is too old, ignore it
        if message_id < self.most_recent_message_id {
            return;
        }

        // update the most recent message id
        if message_id > self.most_recent_message_id {
            self.most_recent_message_id = message_id;
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            entry.insert(message);
        }
    }
    fn read_message(&mut self) -> Option<Message> {
        // keep popping messages until we get one that is more recent than the last one we processed
        loop {
            let Some((message_id, message)) = self.recv_message_buffer.pop_first() else {
                return None;
            };
            if message_id >= self.most_recent_message_id {
                return Some(message);
            }
        }
    }
}
