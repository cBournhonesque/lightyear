use std::collections::VecDeque;

use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender};

use crate::channel::senders::fragment_ack_receiver::FragmentAckReceiver;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{FragmentData, MessageAck, MessageId, SingleData};
use crate::shared::ping::manager::PingManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::{TimeManager, WrappedTime};

const DISCARD_AFTER: chrono::Duration = chrono::Duration::milliseconds(3000);

/// A sender that simply sends the messages without applying any reliability or unordered
/// Same as UnorderedUnreliableSender, but includes a message id to each message,
/// Which can let us track if a message was acked
pub struct UnorderedUnreliableWithAcksSender {
    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SingleData>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<FragmentData>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,

    // TODO: add this to reliable as well?
    // TODO: use a crate to broadcast to all subscribers?
    /// List of senders that want to be notified when a message is acked
    ack_senders: Vec<Sender<MessageId>>,
    /// Keep track of which fragments were acked, so we can know when the entire fragment message
    /// was acked
    fragment_ack_receiver: FragmentAckReceiver,
    current_time: WrappedTime,
}

impl UnorderedUnreliableWithAcksSender {
    pub(crate) fn new() -> Self {
        Self {
            single_messages_to_send: VecDeque::new(),
            fragmented_messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId::default(),
            fragment_sender: FragmentSender::new(),
            ack_senders: Vec::new(),
            fragment_ack_receiver: FragmentAckReceiver::new(),
            current_time: WrappedTime::default(),
        }
    }
}

impl ChannelSend for UnorderedUnreliableWithAcksSender {
    fn update(&mut self, time_manager: &TimeManager, _: &PingManager, _: &TickManager) {
        self.current_time = time_manager.current_time();
        self.fragment_ack_receiver
            .cleanup(self.current_time - DISCARD_AFTER);
    }

    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes, priority: f32) -> Option<MessageId> {
        let message_id = self.next_send_message_id;
        if message.len() > self.fragment_sender.fragment_size {
            let fragments = self
                .fragment_sender
                .build_fragments(message_id, None, message, priority);
            self.fragment_ack_receiver
                .add_new_fragment_to_wait_for(message_id, fragments.len());
            for fragment in fragments {
                self.fragmented_messages_to_send.push_back(fragment);
            }
        } else {
            let single_data = SingleData::new(Some(message_id), message, priority);
            self.single_messages_to_send.push_back(single_data);
        }
        self.next_send_message_id += 1;
        Some(message_id)
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

    /// Notify any subscribers that a message was acked
    fn notify_message_delivered(&mut self, ack: &MessageAck) {
        ack.fragment_id.map_or_else(
            || {
                for sender in &self.ack_senders {
                    sender.send(ack.message_id).unwrap();
                }
            },
            |fragment_index| {
                if self.fragment_ack_receiver.receive_fragment_ack(
                    ack.message_id,
                    fragment_index,
                    None,
                ) {
                    for sender in &self.ack_senders {
                        sender.send(ack.message_id).unwrap();
                    }
                }
            },
        );
    }

    fn has_messages_to_send(&self) -> bool {
        !self.single_messages_to_send.is_empty() || !self.fragmented_messages_to_send.is_empty()
    }

    /// Create a new receiver that will receive a message id when a message is acked
    fn subscribe_acks(&mut self) -> Receiver<MessageId> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        self.ack_senders.push(sender);
        receiver
    }
}

#[cfg(test)]
mod tests {
    use crate::packet::packet::FRAGMENT_SIZE;

    use super::*;

    #[test]
    fn test_receive_ack() {
        let mut sender = UnorderedUnreliableWithAcksSender::new();

        // create subscriber
        let receiver = sender.subscribe_acks();

        // single message
        let message_id = sender.buffer_send(Bytes::from("hello"), 1.0).unwrap();
        assert_eq!(message_id, MessageId(0));
        assert_eq!(sender.next_send_message_id, MessageId(1));

        // ack it
        sender.notify_message_delivered(&MessageAck {
            message_id,
            fragment_id: None,
        });
        assert_eq!(receiver.try_recv().unwrap(), message_id);

        // fragment message
        const NUM_BYTES: usize = (FRAGMENT_SIZE as f32 * 1.5) as usize;
        let bytes = Bytes::from(vec![0; NUM_BYTES]);
        let message_id = sender.buffer_send(bytes, 1.0).unwrap();
        assert_eq!(message_id, MessageId(1));
        let mut expected = FragmentAckReceiver::new();
        expected.add_new_fragment_to_wait_for(message_id, 2);
        assert_eq!(&sender.fragment_ack_receiver, &expected);

        sender.notify_message_delivered(&MessageAck {
            message_id,
            fragment_id: Some(0),
        });
        assert!(receiver.try_recv().unwrap_err().is_empty());
        sender.notify_message_delivered(&MessageAck {
            message_id,
            fragment_id: Some(1),
        });
        assert_eq!(receiver.try_recv().unwrap(), message_id);
    }
}
