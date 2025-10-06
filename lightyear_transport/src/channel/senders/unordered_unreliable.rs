use alloc::collections::VecDeque;
use bevy_time::{Real, Time, Timer, TimerMode};
use core::time::Duration;

use crate::channel::senders::ChannelSend;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::packet::message::{MessageAck, MessageData, MessageId, SendMessage, SingleData};
use bytes::Bytes;
use lightyear_link::LinkStats;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
#[derive(Debug)]
pub struct UnorderedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<SendMessage>,
    /// Fragmented messages need an id (so they can be reconstructed), this keeps track
    /// of the next id to use
    next_send_fragmented_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
}

impl UnorderedUnreliableSender {
    pub(crate) fn new(send_frequency: Duration) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_fragmented_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            timer,
        }
    }
}

impl ChannelSend for UnorderedUnreliableSender {
    fn update(&mut self, real_time: &Time<Real>, _: &LinkStats) {
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
        }
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self
                .fragment_sender
                .build_fragments(self.next_send_fragmented_message_id, message)
            {
                self.fragmented_messages_to_send.push_back(SendMessage {
                    data: MessageData::Fragment(fragment),
                    priority,
                });
            }
            self.next_send_fragmented_message_id += 1;
            Some(self.next_send_fragmented_message_id - 1)
        } else {
            let single_data = SingleData::new(None, message);
            self.single_messages_to_send.push_back(SendMessage {
                data: MessageData::Single(single_data),
                priority,
            });
            None
        }
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets to be sent
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>) {
        if self.timer.as_ref().is_some_and(|t| !t.is_finished()) {
            return (VecDeque::new(), VecDeque::new());
        }
        (
            core::mem::take(&mut self.single_messages_to_send),
            core::mem::take(&mut self.fragmented_messages_to_send),
        )
        // let messages_to_send = core::mem::take(&mut self.messages_to_send);
        // let (remaining_messages_to_send, _) =
        //     packet_manager.pack_messages_within_channel(messages_to_send);
        // self.messages_to_send = remaining_messages_to_send;
    }

    fn receive_ack(&mut self, _: &MessageAck) {}
}

#[cfg(test)]
mod tests {
    // #[test]
    // fn test_unordered_unreliable_sender_internals() {
    //     todo!()
    // }
}
