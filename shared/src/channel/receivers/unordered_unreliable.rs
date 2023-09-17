use crate::channel::receivers::ChannelReceiver;
use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;
use std::collections::VecDeque;
use std::mem;

pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<Message>,
}

impl ChannelReceiver for UnorderedUnreliableReceiver {
    fn buffer_recv(&mut self, message: Message, message_id: MessageId) {
        self.recv_message_buffer.push_back(message);
    }

    fn read_message(&mut self) -> Option<Message> {
        self.recv_message_buffer.pop_front()
    }
}
