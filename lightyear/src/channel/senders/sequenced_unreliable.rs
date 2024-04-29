use std::collections::VecDeque;

use bytes::Bytes;
use crossbeam_channel::Receiver;

use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::TimeManager;

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information
pub struct SequencedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SingleData>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<FragmentData>,

    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
}

impl SequencedUnreliableSender {
    pub(crate) fn new() -> Self {
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId(0),
            fragment_sender: FragmentSender::new(),
        }
    }
}

impl ChannelSend for SequencedUnreliableSender {
    fn update(&mut self, _: &TimeManager, _: &PingManager, _: &TickManager) {}

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        let message_id = self.next_send_message_id;
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self
                .fragment_sender
                .build_fragments(message_id, None, message, priority)
            {
                self.fragmented_messages_to_send.push_back(fragment);
            }
        } else {
            let single_data = SingleData::new(Some(message_id), message, priority);
            self.single_messages_to_send.push_back(single_data);
        }
        self.next_send_message_id += 1;
        Some(message_id)
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
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

    fn notify_message_delivered(&mut self, _message_ack: &MessageAck) {}

    fn has_messages_to_send(&self) -> bool {
        !self.single_messages_to_send.is_empty() || !self.fragmented_messages_to_send.is_empty()
    }

    fn subscribe_acks(&mut self) -> Receiver<MessageId> {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    // #[test]
    // fn test_sequenced_unreliable_sender_internals() {
    //     todo!()
    // }
}
