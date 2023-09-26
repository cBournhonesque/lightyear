use crate::channel::receivers::ChannelReceive;
use crate::packet::message::Message;

use std::collections::VecDeque;


pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<Message>,
}

impl UnorderedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
        }
    }
}

impl ChannelReceive for UnorderedUnreliableReceiver {
    fn buffer_recv(&mut self, message: Message) -> anyhow::Result<()> {
        self.recv_message_buffer.push_back(message);
        Ok(())
    }

    fn read_message(&mut self) -> Option<Message> {
        self.recv_message_buffer.pop_front()
    }
}
