use std::collections::VecDeque;

use anyhow::anyhow;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, MessageId, SingleData};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::{TimeManager, WrappedTime};

const DISCARD_AFTER: chrono::Duration = chrono::Duration::milliseconds(3000);

/// Sequenced Unreliable receiver:
/// do not return messages in order, but ignore the messages that are older than the most recent one received
pub struct SequencedUnreliableReceiver {
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: VecDeque<SingleData>,
    /// Highest message id received so far
    most_recent_message_id: MessageId,
    fragment_receiver: FragmentReceiver,
    current_time: WrappedTime,
}

impl SequencedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
            most_recent_message_id: MessageId(0),
            fragment_receiver: FragmentReceiver::new(),
            // TODO: starting at 0 time could be dangerous, because the first update will bring it to time_manager time ?
            current_time: WrappedTime::default(),
        }
    }
}

impl ChannelReceive for SequencedUnreliableReceiver {
    fn update(&mut self, time_manager: &TimeManager, _: &TickManager) {
        self.current_time = time_manager.current_time();
        self.fragment_receiver
            .cleanup(self.current_time - DISCARD_AFTER);
    }

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
        match message {
            MessageContainer::Single(data) => self.recv_message_buffer.push_back(data),
            MessageContainer::Fragment(data) => {
                if let Some(single_data) = self
                    .fragment_receiver
                    .receive_fragment(data, Some(self.current_time))?
                {
                    self.recv_message_buffer.push_back(single_data);
                }
            }
        }
        Ok(())
    }
    fn read_message(&mut self) -> Option<SingleData> {
        self.recv_message_buffer.pop_front()
        // TODO: naia does a more optimized version by return a Vec<Message> instead of Option<Message>
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::sequenced_unreliable::SequencedUnreliableReceiver;
    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::{MessageId, SingleData};

    #[test]
    fn test_sequenced_unreliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = SequencedUnreliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"), 1.0);
        let mut single2 = SingleData::new(None, Bytes::from("world"), 1.0);
        let mut single3 = SingleData::new(None, Bytes::from("!"), 1.0);

        // receive an old message: it doesn't get added to the buffer
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(single2.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(single2.clone().into())?;

        // the message has been buffered, and we can read it instantly
        // since it's the most recent
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.most_recent_message_id, MessageId(1));
        assert_eq!(receiver.read_message(), Some(single2.clone()));
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive an earlier message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(single1.clone().into())?;
        // we don't add it to the buffer since we have read a more recent message.
        assert_eq!(receiver.recv_message_buffer.len(), 0);
        assert_eq!(receiver.read_message(), None);

        // receive a later message
        single3.id = Some(MessageId(2));
        receiver.buffer_recv(single3.clone().into())?;
        // it's the most recent message so we receive it
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.most_recent_message_id, MessageId(2));
        assert_eq!(receiver.read_message(), Some(single3.clone()));
        assert_eq!(receiver.recv_message_buffer.len(), 0);
        Ok(())
    }
}
