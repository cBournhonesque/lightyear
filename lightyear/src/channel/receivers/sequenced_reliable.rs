use alloc::collections::{btree_map, BTreeMap};

use bytes::Bytes;

use super::error::{ChannelReceiveError, Result};

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};
use crate::prelude::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Sequenced Reliable receiver: make sure that all messages are received,
/// do not return them in order, but ignore the messages that are older than the most recent one received
#[derive(Debug)]
pub struct SequencedReliableReceiver {
    // TODO: optimize via ring buffer?
    // TODO: actually do we even need a buffer? we might just need a buffer of 1
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, (Tick, Bytes)>,
    /// Highest message id received so far
    most_recent_message_id: MessageId,
    fragment_receiver: FragmentReceiver,
}

impl SequencedReliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: BTreeMap::new(),
            most_recent_message_id: MessageId(0),
            fragment_receiver: FragmentReceiver::new(),
        }
    }
}

impl ChannelReceive for SequencedReliableReceiver {
    fn update(&mut self, _: &TimeManager, _: &TickManager) {}

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<()> {
        let message_id = message
            .data
            .message_id()
            .ok_or(ChannelReceiveError::MissingMessageId)?;

        // if the message is too old, ignore it
        if message_id < self.most_recent_message_id {
            return Ok(());
        }

        // update the most recent message id
        if message_id > self.most_recent_message_id {
            self.most_recent_message_id = message_id;
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
    fn read_message(&mut self) -> Option<(Tick, Bytes)> {
        // keep popping messages until we get one that is more recent than the last one we processed
        loop {
            let (message_id, message) = self.recv_message_buffer.pop_first()?;
            if message_id >= self.most_recent_message_id {
                return Some(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::SingleData;

    use super::*;

    // TODO: check that the fragment receiver correctly removes items from the buffer, so they dont accumulate!

    #[test]
    fn test_ordered_reliable_receiver_internals() -> Result<()> {
        let mut receiver = SequencedReliableReceiver::new();

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

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.contains_key(&MessageId(1)));
        assert_eq!(
            receiver.read_message(),
            Some((Tick(2), single2.bytes.clone()))
        );
        assert_eq!(receiver.most_recent_message_id, MessageId(1));

        // receive message 0:
        // we don't care about receiving message 0 anymore, since we already have received a more recent message
        // gets discarded
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(ReceiveMessage {
            data: single1.clone().into(),
            remote_sent_tick: Tick(3),
        })?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);
        assert_eq!(receiver.read_message(), None);
        Ok(())
    }
}
