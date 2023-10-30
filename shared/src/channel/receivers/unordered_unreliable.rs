use crate::BitSerializable;
use std::collections::VecDeque;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, SingleData};
use crate::packet::packet::FragmentData;
use crate::packet::wrapping_id::MessageId;

pub struct UnorderedUnreliableReceiver {
    recv_message_buffer: VecDeque<SingleData>,
    fragment_receiver: FragmentReceiver,
}

impl UnorderedUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: VecDeque::new(),
            fragment_receiver: FragmentReceiver::new(),
        }
    }
}

impl ChannelReceive for UnorderedUnreliableReceiver {
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        match message {
            MessageContainer::Single(data) => self.recv_message_buffer.push_back(data),
            MessageContainer::Fragment(fragment) => {
                if let Some(data) = self.fragment_receiver.receive_fragment(fragment)? {
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
