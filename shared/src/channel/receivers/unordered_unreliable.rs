use crate::channel::receivers::ChannelReceive;
use crate::packet::message::MessageContainer;

use std::collections::VecDeque;

pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<MessageContainer>,
}

impl UnorderedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
        }
    }
}

impl ChannelReceive for UnorderedUnreliableReceiver {
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        self.recv_message_buffer.push_back(message);
        Ok(())
    }

    fn read_message(&mut self) -> Option<MessageContainer> {
        self.recv_message_buffer.pop_front()
    }
}
