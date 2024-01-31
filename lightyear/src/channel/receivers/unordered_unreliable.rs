use std::collections::VecDeque;
use tracing::info;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, SingleData};
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::{TimeManager, WrappedTime};

const DISCARD_AFTER: chrono::Duration = chrono::Duration::milliseconds(3000);

pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<SingleData>,
    fragment_receiver: FragmentReceiver,
    current_time: WrappedTime,
}

impl UnorderedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
            fragment_receiver: FragmentReceiver::new(),
            current_time: WrappedTime::default(),
        }
    }
}

impl ChannelReceive for UnorderedUnreliableReceiver {
    fn update(&mut self, time_manager: &TimeManager, _: &TickManager) {
        self.current_time = time_manager.current_time();
        self.fragment_receiver
            .cleanup(self.current_time - DISCARD_AFTER);
    }

    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        match message {
            MessageContainer::Single(data) => self.recv_message_buffer.push_back(data),
            MessageContainer::Fragment(fragment) => {
                if let Some(data) = self
                    .fragment_receiver
                    .receive_fragment(fragment, Some(self.current_time))?
                {
                    self.recv_message_buffer.push_back(data);
                }
            }
        }
        Ok(())
    }

    fn read_message(&mut self) -> Option<SingleData> {
        self.recv_message_buffer.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::{MessageId, SingleData};

    use super::*;

    #[test]
    fn test_unordered_unreliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = UnorderedUnreliableReceiver::new();

        let mut single1 = SingleData::new(None, Bytes::from("hello"), 1.0);
        let mut single2 = SingleData::new(None, Bytes::from("world"), 1.0);

        // receive an old message
        single2.id = Some(MessageId(60000));
        receiver.buffer_recv(single2.clone().into())?;

        // it still gets read
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.read_message(), Some(single2.clone()));

        // receive message in the wrong order
        single2.id = Some(MessageId(1));
        receiver.buffer_recv(single2.clone().into())?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.read_message(), Some(single2.clone()));

        // receive message 0
        single1.id = Some(MessageId(0));
        receiver.buffer_recv(single1.clone().into())?;

        // we process the message
        assert_eq!(receiver.recv_message_buffer.len(), 1);
        assert_eq!(receiver.read_message(), Some(single1.clone()));
        Ok(())
    }
}
