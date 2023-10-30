use bytes::Bytes;
use std::collections::{BTreeMap, HashSet};
#[cfg(not(test))]
use std::time::Instant;
use std::{collections::VecDeque, time::Duration};

#[cfg(test)]
use mock_instant::Instant;

use crate::channel::channel::ReliableSettings;
use crate::channel::senders::fragment_sender::FragmentSender;
use crate::channel::senders::ChannelSend;
use crate::packet::message::{MessageContainer, SingleData};
use crate::packet::packet::FragmentData;
use crate::packet::packet_manager::PacketManager;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::BitSerializable;

pub struct FragmentAck {
    data: FragmentData,
    acked: bool,
    last_sent: Option<Instant>,
}

/// A message that has not been acked yet
pub enum UnackedMessage {
    Single {
        bytes: Bytes,
        /// If None: this packet has never been sent before
        /// else: the last instant when this packet was sent
        last_sent: Option<Instant>,
    },
    Fragmented(Vec<FragmentAck>),
}

/// A sender that makes sure to resend messages until it receives an ack
pub struct ReliableSender {
    /// Settings for reliability
    reliable_settings: ReliableSettings,
    // TODO: maybe optimize by using a RingBuffer
    /// Ordered map of the messages that haven't been acked yet
    unacked_messages: BTreeMap<MessageId, UnackedMessage>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,

    /// list of single messages that we want to fit into packets and send
    single_messages_to_send: VecDeque<SingleData>,
    /// list of fragmented messages that we want to fit into packets and send
    fragmented_messages_to_send: VecDeque<FragmentData>,

    /// Set of message ids that we want to send (to prevent sending the same message twice)
    message_ids_to_send: HashSet<MessageId>,

    /// Used to split a message into fragments if the message is too big
    fragment_sender: FragmentSender,

    current_rtt_millis: f32,
    current_time: Instant,
}

impl ReliableSender {
    pub fn new(reliable_settings: ReliableSettings) -> Self {
        Self {
            reliable_settings,
            unacked_messages: Default::default(),
            next_send_message_id: MessageId(0),
            single_messages_to_send: Default::default(),
            fragmented_messages_to_send: Default::default(),
            message_ids_to_send: Default::default(),
            fragment_sender: FragmentSender::new(),
            current_rtt_millis: 0.0,
            current_time: Instant::now(),
        }
    }

    /// Called when we receive an ack that a message that we sent has been received
    fn process_message_ack(&mut self, message_id: MessageId) {
        if self.unacked_messages.contains_key(&message_id) {
            self.unacked_messages.remove(&message_id).unwrap();
        }
    }
}

// Stragegy:
// - a Message is a single unified data structure that knows how to serialize itself
// - a Packet can be a single packet, or a multi-fragment slice, or a single fragment of a slice (i.e. a fragment that needs to be resent)
// - all messages know how to serialize themselves into a packet or a list of packets to send over the wire.
//   that means they have the information to create their header (i.e. their PacketId or FragmentId)
// - SEND = get a list of Messages to send
// (either packets in the buffer, or packets we need to resend cuz they were not acked,
// or because one of the fragments of the )
// - (because once we have that list, that list knows how to serialize itself)
impl ChannelSend for ReliableSender {
    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Bytes) {
        let unacked_message = if message.len() > self.fragment_sender.fragment_size {
            let fragments = self
                .fragment_sender
                .build_fragments(self.next_send_message_id, message);

            UnackedMessage::Fragmented(
                fragments
                    .into_iter()
                    .map(|fragment| FragmentAck {
                        data: fragment,
                        acked: false,
                        last_sent: None,
                    })
                    .collect(),
            )
        } else {
            UnackedMessage::Single {
                bytes: message,
                last_sent: None,
            }
        };
        self.unacked_messages
            .insert(self.next_send_message_id, unacked_message);
        self.next_send_message_id += 1;
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    /// The messages to be sent need to have been collected prior to this point.
    fn send_packet(&mut self) -> (VecDeque<SingleData>, VecDeque<FragmentData>) {
        (
            std::mem::take(&mut self.single_messages_to_send),
            std::mem::take(&mut self.fragmented_messages_to_send),
        )

        // // build the packets from those messages
        // let single_messages_to_send = std::mem::take(&mut self.single_messages_to_send);
        // let (remaining_messages_to_send, sent_message_ids) =
        //     packet_manager.pack_messages_within_channel(messages_to_send);
        // self.messages_to_send = remaining_messages_to_send;
        //
        // for message_id in sent_message_ids {
        //     self.message_ids_to_send.remove(&message_id);
        // }
    }

    /// Collect the list of messages that need to be sent
    /// Either because they have never been sent, or because they need to be resent
    /// Needs to be called before [`ReliableSender::send_packet`]
    fn collect_messages_to_send(&mut self) {
        // resend delay is based on the rtt
        let resend_delay = Duration::from_millis(
            (self.reliable_settings.rtt_resend_factor * self.current_rtt_millis) as u64,
        );
        let should_send = |last_sent: Option<Instant>| -> bool {
            match last_sent {
                // send it the message has never been sent
                None => true,
                // or if we sent it a while back but didn't get an ack
                Some(last_sent) => self.current_time - last_sent > resend_delay,
            }
        };

        // Iterate through all unacked messages, oldest message ids first
        for (message_id, unacked_message) in self.unacked_messages.iter_mut() {
            match unacked_message {
                UnackedMessage::Single {
                    bytes,
                    mut last_sent,
                } => {
                    if should_send(last_sent) {
                        // TODO: this is a vecdeque, so if we call this function multiple times
                        //  we would send the same message multiple times.  Use HashSet<MessageId> to prevent this?
                        // if !self.message_ids_to_send.contains(message_id) {
                        let message = SingleData::new(Some(*message_id), bytes.clone()).into();
                        self.single_messages_to_send.push_back(message);
                        // self.message_ids_to_send.insert(*message_id);
                        last_sent = Some(self.current_time);
                        // }
                    }
                }
                UnackedMessage::Fragmented(fragment_acks) => {
                    // only send the fragments that haven't been acked and should be resent
                    fragment_acks
                        .iter_mut()
                        .filter(|f| !f.acked && should_send(f.last_sent))
                        .for_each(|f| {
                            // TODO: need a mechanism like message_ids_to_send? (message/fragmnet_id) to send?
                            let message = f.data.clone().into();
                            self.fragmented_messages_to_send.push_back(message);
                            f.last_sent = Some(self.current_time);
                        })
                }
            }
        }
    }

    // TODO: need message_delivered to contain possibly fragment information!!!
    fn notify_message_delivered(&mut self, message_id: &MessageId) {
        self.unacked_messages.remove(message_id);
    }

    fn has_messages_to_send(&self) -> bool {
        !self.messages_to_send.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mock_instant::MockClock;

    use crate::channel::channel::ReliableSettings;

    use super::ChannelSend;
    use super::Instant;
    use super::ReliableSender;
    use super::{MessageContainer, MessageId};

    #[test]
    fn test_reliable_sender_internals() {
        let mut sender = ReliableSender {
            reliable_settings: ReliableSettings {
                rtt_resend_factor: 1.5,
            },
            unacked_messages: Default::default(),
            next_send_message_id: MessageId(0),
            messages_to_send: Default::default(),
            message_ids_to_send: Default::default(),
            current_rtt_millis: 100.0,
            current_time: Instant::now(),
        };

        // Buffer a new message
        let mut message = MessageContainer::new(1);
        sender.buffer_send(message.clone());
        assert_eq!(sender.unacked_messages.len(), 1);
        assert_eq!(sender.next_send_message_id, MessageId(1));
        // Collect the messages to be sent
        sender.collect_messages_to_send();
        assert_eq!(sender.messages_to_send.len(), 1);

        // Advance by a time that is below the resend threshold
        MockClock::advance(Duration::from_millis(100));
        sender.current_time = Instant::now();
        sender.collect_messages_to_send();
        assert_eq!(sender.messages_to_send.len(), 1);

        // Advance by a time that is above the resend threshold
        MockClock::advance(Duration::from_millis(200));
        sender.current_time = Instant::now();
        sender.collect_messages_to_send();
        assert_eq!(sender.messages_to_send.len(), 1);
        message.id = Some(MessageId(0));
        assert_eq!(sender.messages_to_send.get(0).unwrap(), &message.clone());

        // Ack the first message
        sender.process_message_ack(MessageId(0));
        assert_eq!(sender.unacked_messages.len(), 0);

        // Advance by a time that is above the resend threshold
        MockClock::advance(Duration::from_millis(200));
        sender.current_time = Instant::now();
        // this time there are no new messages to send
        assert_eq!(sender.messages_to_send.len(), 1);
    }
}
