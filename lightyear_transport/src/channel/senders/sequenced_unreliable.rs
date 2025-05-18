use crate::channel::senders::ChannelSend;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::packet::message::{MessageAck, MessageData, MessageId, SendMessage, SingleData};
use alloc::collections::VecDeque;
use bevy::prelude::{Real, Time};
use bevy::time::{Timer, TimerMode};
use bytes::Bytes;
use core::time::Duration;
use lightyear_link::LinkStats;

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information (MessageId)
#[derive(Debug)]
pub struct SequencedUnreliableSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<SendMessage>,

    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
}

impl SequencedUnreliableSender {
    pub(crate) fn new(send_frequency: Duration) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId(0),
            fragment_sender: FragmentSender::new(),
            timer,
        }
    }
}

impl ChannelSend for SequencedUnreliableSender {
    fn update(&mut self, real_time: &Time<Real>, _: &LinkStats) {
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
        }
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        let message_id = self.next_send_message_id;
        if message.len() > self.fragment_sender.fragment_size {
            for fragment in self.fragment_sender.build_fragments(message_id, message) {
                self.fragmented_messages_to_send.push_back(SendMessage {
                    data: MessageData::Fragment(fragment),
                    priority,
                });
            }
        } else {
            let single_data = SingleData::new(Some(message_id), message);
            self.single_messages_to_send.push_back(SendMessage {
                data: MessageData::Single(single_data),
                priority,
            });
        }
        self.next_send_message_id += 1;
        Some(message_id)
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self) -> (VecDeque<SendMessage>, VecDeque<SendMessage>) {
        if self.timer.as_ref().is_some_and(|t| !t.finished()) {
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

    fn receive_ack(&mut self, _message_ack: &MessageAck) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_sequenced_unreliable_sender_internals() {
        let mut sender = SequencedUnreliableSender::new(Duration::from_secs(1));
        assert!(sender.timer.as_ref().is_some_and(|t| !t.finished()));

        sender.buffer_send(Bytes::from("hello"), 1.0).unwrap();

        // we do not send because we didn't reach the timer
        let (single, _) = sender.send_packet();
        assert!(single.is_empty());

        // update with a delta of 1 second
        let mut real = Time::<Real>::default();
        real.advance_by(Duration::from_secs(1));
        let link_stats = LinkStats::default();
        sender.update(&real, &link_stats);

        // this time, we send the packet
        let (single, _) = sender.send_packet();
        assert_eq!(single.len(), 1);
    }
}
