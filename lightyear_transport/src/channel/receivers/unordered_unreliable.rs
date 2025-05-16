use crate::channel::receivers::error::ChannelReceiveError;
use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageData, MessageId, ReceiveMessage};
use alloc::collections::VecDeque;
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::tick::Tick;

const DISCARD_AFTER: Duration = Duration::from_millis(3000);

#[derive(Debug)]
pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<(Tick, Bytes)>,
    fragment_receiver: FragmentReceiver,
    current_time: Duration,
}

impl Default for UnorderedUnreliableReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl UnorderedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
            fragment_receiver: FragmentReceiver::new(),
            current_time: Duration::default(),
        }
    }
}

impl ChannelReceive for UnorderedUnreliableReceiver {
    fn update(&mut self, now: Duration) {
        self.current_time = now;
        self.fragment_receiver
            .cleanup(self.current_time.saturating_sub(DISCARD_AFTER));
    }

    fn buffer_recv(&mut self, message: ReceiveMessage) -> Result<(), ChannelReceiveError> {
        match message.data {
            MessageData::Single(single) => self
                .recv_message_buffer
                .push_back((message.remote_sent_tick, single.bytes)),
            // TODO: which tick is used when multiple fragments are received?
            MessageData::Fragment(fragment) => {
                if let Some(data) = self.fragment_receiver.receive_fragment(
                    fragment,
                    message.remote_sent_tick,
                    Some(self.current_time),
                ) {
                    self.recv_message_buffer.push_back(data);
                }
            }
        }
        Ok(())
    }

    fn read_message(&mut self) -> Option<(Tick, Bytes, Option<MessageId>)> {
        self.recv_message_buffer
            .pop_front()
            .map(|(tick, bytes)| (tick, bytes, None))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::error::ChannelReceiveError;
    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::{MessageId, SingleData};

    use super::*;

    #[test]
    fn test_unordered_unreliable_receiver_internals() -> Result<(), ChannelReceiveError> {
        let mut receiver = UnorderedUnreliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"));
        let mut single2 = SingleData::new(None, Bytes::from("world"));

        // receive an old message
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(ReceiveMessage {
            data: single2.clone().into(),
            remote_sent_tick: Tick(1),
        })?;

        // it still gets read
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(
            receiver.read_message(),
            Some((Tick(1), single2.bytes.clone(), None))
        );

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(ReceiveMessage {
            data: single2.clone().into(),
            remote_sent_tick: Tick(2),
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(
            receiver.read_message(),
            Some((Tick(2), single2.bytes.clone(), None))
        );

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(ReceiveMessage {
            data: single1.clone().into(),
            remote_sent_tick: Tick(3),
        })?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(
            receiver.read_message(),
            Some((Tick(3), single1.bytes.clone(), None))
        );
        Ok(())
    }
}
