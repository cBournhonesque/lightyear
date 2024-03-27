use std::collections::{btree_map, BTreeMap};

use anyhow::anyhow;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, MessageId, SingleData};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// Sequenced Reliable receiver: make sure that all messages are received,
/// do not return them in order, but ignore the messages that are older than the most recent one received
pub struct SequencedReliableReceiver {
    // TODO: optimize via ring buffer?
    // TODO: actually do we even need a buffer? we might just need a buffer of 1
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: BTreeMap<MessageId, SingleData>,
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
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        let message_id = message
            .message_id()
            .ok_or_else(|| anyhow!("message id not found"))?;

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
    fn read_message(&mut self) -> Option<SingleData> {
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
    fn test_ordered_reliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = SequencedReliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"), 1.0);
        let mut single2 = SingleData::new(None, Bytes::from("world"), 1.0);

        // receive an old message: it doesn't get added to the buffer because the next one we expect is 0
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(single2.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(single2.clone().into())?;

        // we process the message

        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(receiver.recv_message_buffer.get(&MessageId(1)).is_some());
        assert_eq!(receiver.read_message(), Some(single2.clone()));
        assert_eq!(receiver.most_recent_message_id, MessageId(1));

        // receive message 0:
        // we don't care about receiving message 0 anymore, since we already have received a more recent message
        // gets discarded
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(single1.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);
        assert_eq!(receiver.read_message(), None);
        Ok(())
    }
}
