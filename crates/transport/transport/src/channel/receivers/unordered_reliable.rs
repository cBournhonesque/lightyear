use alloc::collections::VecDeque;
use bevy_platform::collections::HashSet;
use core::time::Duration;

use super::error::ChannelReceiveError;
use bytes::Bytes;
use lightyear_core::tick::Tick;
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::channel::receivers::ChannelReceive;
use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};

/// Unordered Reliable receiver: make sure that all messages are received,
/// and return them in any order
#[derive(Debug)]
pub struct UnorderedReliableReceiver {
    /// Next message id that we are waiting to receive
    /// The channel is reliable so we should see all message ids.
    pending_recv_message_id: MessageId,
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: VecDeque<(Tick, Bytes, MessageId)>,
    fragment_receiver: FragmentReceiver,
    /// Keep tracking of the message ids we have received, so we can update the oldest_pending_message_id
    received_message_ids: HashSet<MessageId>,
}

impl Default for UnorderedReliableReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl UnorderedReliableReceiver {
    pub fn new() -> Self {
        Self {
            pending_recv_message_id: MessageId(0),
            recv_message_buffer: VecDeque::new(),
            fragment_receiver: FragmentReceiver::new(),
            received_message_ids: HashSet::default(),
        }
    }
}

impl ChannelReceive for UnorderedReliableReceiver {
    fn set_fragment_size(&mut self, fragment_size: usize) {
        self.fragment_receiver.set_fragment_size(fragment_size);
    }

    fn update(&mut self, _: Duration) {}

    /// Queues a received message in an internal buffer
    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<(), ChannelReceiveError> {
        let message_id = message
            .data
            .message_id()
            .ok_or(ChannelReceiveError::MissingMessageId)?;
        trace!("receiving unordered reliable message id: {message_id:?}");

        // we have already received the message if it's older than the oldest pending message
        // (since we are reliable, we should have received all messages prior to that one)
        if message_id < self.pending_recv_message_id {
            trace!(
                "ignore message {message_id:?} since its older than pending {:?}",
                self.pending_recv_message_id
            );
            return Ok(());
        }

        if self.received_message_ids.contains(&message_id) {
            return Ok(());
        }

        match message.data {
            MessageData::Single(single) => {
                if self.received_message_ids.insert(message_id) {
                    self.recv_message_buffer.push_back((
                        message.remote_sent_tick,
                        single.bytes,
                        message_id,
                    ));
                }
            }
            MessageData::Fragment(fragment) => {
                if let Some((tick, bytes)) = self.fragment_receiver.receive_fragment(
                    fragment,
                    message.remote_sent_tick,
                    None,
                    message.compression,
                )? && self.received_message_ids.insert(message_id)
                {
                    self.recv_message_buffer
                        .push_back((tick, bytes, message_id));
                }
            }
        }
        Ok(())
    }

    fn read_message(&mut self) -> Option<(Tick, Bytes, Option<MessageId>)> {
        // return if there are no messages in the buffer
        let (tick, bytes, message_id) = self.recv_message_buffer.pop_front()?;

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

        Some((tick, bytes, Some(message_id)))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::ChannelReceive;
    use crate::packet::compression::CompressionConfig;
    use crate::packet::message::SingleData;

    use super::*;

    #[test]
    fn test_unordered_reliable_receiver_internals() -> Result<(), ChannelReceiveError> {
        let mut receiver = UnorderedReliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"));
        let mut single2 = SingleData::new(None, Bytes::from("world"));
        let mut stale = SingleData::new(None, Bytes::from("stale"));

        // Receive an old message: it doesn't get added to the buffer because the next one we
        // expect is newer.
        receiver.pending_recv_message_id = MessageId(2);
        stale.id = Some(MessageId(1));
        receiver.buffer_recv(ReceiveMessage {
            data: stale.clone().into(),
            remote_sent_tick: Tick(1),
            compression: CompressionConfig::DISABLED,
        })?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        let mut receiver = UnorderedReliableReceiver::new();

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(ReceiveMessage {
            data: single2.clone().into(),
            remote_sent_tick: Tick(3),
            compression: CompressionConfig::DISABLED,
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(
            receiver
                .recv_message_buffer
                .iter()
                .any(|(_, _, message_id)| *message_id == MessageId(1))
        );
        assert_eq!(
            receiver.read_message(),
            Some((Tick(3), single2.bytes.clone(), Some(MessageId(1))))
        );

        // we are still expecting message id 0
        assert_eq!(receiver.pending_recv_message_id, MessageId(0));

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(ReceiveMessage {
            data: single1.clone().into(),
            remote_sent_tick: Tick(5),
            compression: CompressionConfig::DISABLED,
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert!(
            receiver
                .recv_message_buffer
                .iter()
                .any(|(_, _, message_id)| *message_id == MessageId(0))
        );
        assert_eq!(
            receiver.read_message(),
            Some((Tick(5), single1.bytes.clone(), Some(MessageId(0))))
        );
        assert_eq!(receiver.pending_recv_message_id, MessageId(2));
        Ok(())
    }

    #[test]
    fn advances_and_deduplicates_across_message_id_rollover() -> Result<(), ChannelReceiveError> {
        let mut receiver = UnorderedReliableReceiver::new();
        receiver.pending_recv_message_id = MessageId(u32::MAX);

        let after_wrap = SingleData::new(Some(MessageId(0)), Bytes::from("after wrap"));
        receiver.buffer_recv(ReceiveMessage {
            data: after_wrap.clone().into(),
            remote_sent_tick: Tick(1),
            compression: CompressionConfig::DISABLED,
        })?;

        let before_wrap = SingleData::new(Some(MessageId(u32::MAX)), Bytes::from("before wrap"));
        receiver.buffer_recv(ReceiveMessage {
            data: before_wrap.clone().into(),
            remote_sent_tick: Tick(2),
            compression: CompressionConfig::DISABLED,
        })?;

        assert_eq!(
            receiver.read_message(),
            Some((Tick(1), after_wrap.bytes, Some(MessageId(0))))
        );
        assert_eq!(
            receiver.read_message(),
            Some((Tick(2), before_wrap.bytes, Some(MessageId(u32::MAX))))
        );
        assert_eq!(receiver.pending_recv_message_id, MessageId(1));

        receiver.buffer_recv(ReceiveMessage {
            data: SingleData::new(Some(MessageId(u32::MAX)), Bytes::from("duplicate")).into(),
            remote_sent_tick: Tick(3),
            compression: CompressionConfig::DISABLED,
        })?;
        receiver.buffer_recv(ReceiveMessage {
            data: SingleData::new(Some(MessageId(0)), Bytes::from("duplicate")).into(),
            remote_sent_tick: Tick(4),
            compression: CompressionConfig::DISABLED,
        })?;
        assert_eq!(receiver.read_message(), None);
        Ok(())
    }
}
