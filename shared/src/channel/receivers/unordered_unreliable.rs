use crate::channel::receivers::ChannelReceiver;
use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;
use std::collections::VecDeque;
use std::mem;

pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<Message>,
}

impl ChannelReceiver for UnorderedUnreliableReceiver {
    fn buffer_recv(&mut self, message: Message) -> anyhow::Result<()> {
        self.recv_message_buffer.push_back(message);
        Ok(())
    }

    fn read_message(&mut self) -> Option<Message> {
        self.recv_message_buffer.pop_front()
    }
}
