use alloc::collections::{btree_map, BTreeMap};
use bevy::platform::collections::HashSet;

use bytes::Bytes;

use super::error::ChannelReceiveError;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};
use crate::prelude::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Unordered Reliable receiver: make sure that all messages are received,
/// and return them in any order
#[derive(Debug)]
pub struct UnorderedReliableReceiver {
    /// Next message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids.
    pending_recv_message_id: MessageId,
    // TODO: optimize via ring buffer?
    // TODO: actually we could just use a VecDeque here?
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, (Tick, Bytes)>,
    fragment_receiver: FragmentReceiver,
    /// Keep tracking of the message ids we have received, so we can update the oldest_pending_message_id
    received_message_ids: HashSet<MessageId>,
}

impl UnorderedReliableReceiver {
    pub fn new() -> Self {
        Self {
            pending_recv_message_id: MessageId(0),
            recv_message_buffer: BTreeMap::new(),
            fragment_receiver: FragmentReceiver::new(),
            received_message_ids: HashSet::default(),
        }
    }
}

impl ChannelReceive for UnorderedReliableReceiver {
    fn update(&mut self, _: &TimeManager, _: &TickManager) {}

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<(), ChannelReceiveError> {
        let message_id = message
            .data
            .message_id()
            .ok_or(ChannelReceiveError::MissingMessageId)?;

        // we have already received the message if it's older than the oldest pending message
        // (since we are reliable, we should have received all messages prior to that one)
        if message_id < self.pending_recv_message_id {
            return Ok(());
        }

        // add the message to the buffer
        if let btree_map::Entry::Vacant(entry) = self.recv_message_buffer.entry(message_id) {
            match message.data {
                MessageData::Single(single) => {
                    // receive the message if we haven't received it already
                    if !self.received_message_ids.contains(&message_id) {
                        self.received_message_ids.insert(message_id);
                        entry.insert((message.remote_sent_tick, single.bytes));
                    }
                }
                MessageData::Fragment(fragment) => {
                    if let Some(res) = self.fragment_receiver.receive_fragment(
                        fragment,
                        message.remote_sent_tick,
                        None,
                    ) {
                        // receive the message if we haven't received it already
                        if !self.received_message_ids.contains(&message_id) {
                            self.received_message_ids.insert(message_id);
                            entry.insert(res);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn read_message(&mut self) -> Option<(Tick, Bytes)> {
        // return if there are no messages in the buffer
        let (message_id, data) = self.recv_message_buffer.pop_first()?;

        // this was the message we were waiting for (as a reliable receiver)
        if self.pending_recv_message_id == message_id {
            // update the pending message id (skip through all message ids we have already received out of order)
            while self
                .received_message_ids
                .contains(&self.pending_recv_message_id)
            {
                self.received_message_ids
                    .remove(&self.pending_recv_message_id);
                self.pending_recv_message_id += 1;
            }
        }

        // receive oldest message in the buffer
        Some(data)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::SingleData;

    use super::*;

    #[test]
    fn test_unordered_reliable_receiver_internals() -> Result<(), ChannelReceiveError> {
        let mut receiver = UnorderedReliableReceiver::new();

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
            remote_sent_tick: Tick(3),
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.contains_key(&MessageId(1)));
        assert_eq!(
            receiver.read_message(),
            Some((Tick(3), single2.bytes.clone()))
        );

        // we are still expecting message id 0
        assert_eq!(receiver.pending_recv_message_id, MessageId(0));

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(ReceiveMessage {
            data: single1.clone().into(),
            remote_sent_tick: Tick(5),
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.contains_key(&MessageId(0)));
        assert_eq!(
            receiver.read_message(),
            Some((Tick(5), single1.bytes.clone()))
        );
        assert_eq!(receiver.pending_recv_message_id, MessageId(2));
        Ok(())
    }
}
