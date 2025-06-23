use alloc::collections::VecDeque;
use bevy_time::{Real, Time, Timer, TimerMode};
use core::time::Duration;

use crate::channel::senders::ChannelSend;
use crate::channel::senders::fragment_ack_receiver::FragmentAckReceiver;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::packet::message::{MessageAck, MessageData, MessageId, SendMessage, SingleData};
use bytes::Bytes;
use lightyear_link::LinkStats;

const DISCARD_AFTER: Duration = Duration::from_millis(3000);

/// A sender that simply sends the messages without applying any reliability or unordered
/// Same as UnorderedUnreliableSender, but includes a message id to each message,
/// Which can let us track if a message was acked
#[derive(Debug)]
pub struct UnorderedUnreliableWithAcksSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SendMessage>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<SendMessage>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,
    /// Keep track of which fragments were acked, so we can know when the entire fragment message
    /// was acked
    fragment_ack_receiver: FragmentAckReceiver,
    /// Internal timer to determine if the channel is ready to send messages
    timer: Option<Timer>,
}

impl UnorderedUnreliableWithAcksSender {
    pub(crate) fn new(send_frequency: Duration) -> Self {
        let timer = if send_frequency == Duration::default() {
            None
        } else {
            Some(Timer::new(send_frequency, TimerMode::Repeating))
        };
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            fragment_ack_receiver: FragmentAckReceiver::new(),
            timer,
        }
    }
}

impl ChannelSend for UnorderedUnreliableWithAcksSender {
    fn update(&mut self, real_time: &Time<Real>, _: &LinkStats) {
        self.fragment_ack_receiver
            .cleanup(real_time.elapsed().saturating_sub(DISCARD_AFTER));
        if let Some(timer) = &mut self.timer {
            timer.tick(real_time.delta());
        }
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        let message_id = self.next_send_message_id;
        if message.len() > self.fragment_sender.fragment_size {
            let fragments = self.fragment_sender.build_fragments(message_id, message);
            self.fragment_ack_receiver
                .add_new_fragment_to_wait_for(message_id, fragments.len());
            for fragment in fragments {
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

    /// Take messages from the buffer of messages to be sent, and build a list of packets to be sent
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

    /// Notify any subscribers that a message was acked
    fn receive_ack(&mut self, ack: &MessageAck) {
        if let Some(fragment_index) = ack.fragment_id {
            self.fragment_ack_receiver
                .receive_fragment_ack(ack.message_id, fragment_index, None);
        }
    }
}
