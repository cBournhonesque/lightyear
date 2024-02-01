use std::collections::VecDeque;

use bytes::Bytes;
use crossbeam_channel::Receiver;

use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// A sender that simply sends the messages without checking if they were received
/// Acts as unreliable unordered, but sends the current tick with the message.
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
    fn update(&mut self, _: &TimeManager, _: &PingManager, tick_manager: &TickManager) {
        self.current_tick = tick_manager.tick();
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self.fragment_sender.build_fragments(
                self.next_send_fragmented_message_id,
                Some(self.current_tick),
                message,
                priority,
            ) {
                self.fragmented_messages_to_send.push_back(fragment);
            }
            self.next_send_fragmented_message_id += 1;
            Some(self.next_send_fragmented_message_id - 1)
        } else {
            let mut single_data = SingleData::new(None, message, priority);
            single_data.tick = Some(self.current_tick);
            self.single_messages_to_send.push_back(single_data);
            None
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

    fn notify_message_delivered(&mut self, _: &MessageAck) {}

    fn has_messages_to_send(&self) -> bool {
        !self.single_messages_to_send.is_empty() || !self.fragmented_messages_to_send.is_empty()
    }

    fn subscribe_acks(&mut self) -> Receiver<MessageId> {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_tick_unreliable_sender_internals() {
        // let mut sender = UnorderedUnreliableSender::new();
    }
}
