use std::collections::VecDeque;

use crate::channel::receivers::ChannelReceive;
use crate::packet::message::MessageContainer;

pub struct UnorderedUnreliableReceiver<P> {
    recv_message_buffer: VecDeque<MessageContainer<P>>,
}

impl<P> UnorderedUnreliableReceiver<P> {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
        }
    }
}

impl<P> ChannelReceive<P> for UnorderedUnreliableReceiver<P> {
    fn buffer_recv(&mut self, message: MessageContainer<P>) -> anyhow::Result<()> {
        self.recv_message_buffer.push_back(message);
        Ok(())
    }

    fn read_message(&mut self) -> Option<MessageContainer<P>> {
        self.recv_message_buffer.pop_front()
    }
}
