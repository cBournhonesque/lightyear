use bytes::Bytes;
use std::collections::VecDeque;
use std::time::Duration;

use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageContainer, MessageId, SingleData};
use crate::packet::packet::FRAGMENT_SIZE;
use crate::packet::packet_manager::PacketManager;
use crate::protocol::BitSerializable;
use crate::tick::Tick;
use crate::TickManager;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
pub struct TickUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SingleData>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<FragmentData>,
    /// Fragmented messages need an id (so they can be reconstructed), this keeps track
    /// of the next id to use
    next_send_fragmented_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    current_tick: Tick,
}

impl TickUnreliableSender {
    pub(crate) fn new() -> Self {
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_fragmented_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            current_tick: Tick(0),
        }
    }
}

impl ChannelSend for TickUnreliableSender {
    fn update(&mut self, delta: Duration, tick_manager: &TickManager) {
        self.current_tick = tick_manager.current_tick();
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes) {
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self.fragment_sender.build_fragments(
                self.next_send_fragmented_message_id,
                Some(self.current_tick),
                message,
            ) {
                self.fragmented_messages_to_send.push_back(fragment);
            }
            self.next_send_fragmented_message_id += 1;
        } else {
            let mut single_data = SingleData::new(None, message);
            single_data.tick = Some(self.current_tick);
            self.single_messages_to_send.push_back(single_data);
        }
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets to be sent
    fn send_packet(&mut self) -> (VecDeque<SingleData>, VecDeque<FragmentData>) {
        (
            std::mem::take(&mut self.single_messages_to_send),
            std::mem::take(&mut self.fragmented_messages_to_send),
        )
        // let messages_to_send = std::mem::take(&mut self.messages_to_send);
        // let (remaining_messages_to_send, _) =
        //     packet_manager.pack_messages_within_channel(messages_to_send);
        // self.messages_to_send = remaining_messages_to_send;
    }

    // not necessary for an unreliable sender (all the buffered messages can be sent)
    fn collect_messages_to_send(&mut self) {}

    fn notify_message_delivered(&mut self, message_ack: &MessageAck) {}

    fn has_messages_to_send(&self) -> bool {
        !self.single_messages_to_send.is_empty() || !self.fragmented_messages_to_send.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    use crate::packet::message::SingleData;

    use super::ChannelSend;
    use super::{MessageContainer, MessageId};

    #[test]
    fn test_tick_unreliable_sender_internals() {
        // let mut sender = UnorderedUnreliableSender::new();
    }
}
