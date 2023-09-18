use crate::channel::senders::message_packer::MessagePacker;
use crate::channel::senders::ChannelSend;
use crate::packet::message::Message;
use crate::packet::packet::Packet;
use crate::packet::wrapping_id::MessageId;
use std::collections::BTreeMap;
use std::{collections::VecDeque, time::Duration};

use crate::channel::channel::ReliableSettings;
use crate::packet::manager::PacketManager;
#[cfg(test)]
use mock_instant::Instant;
#[cfg(not(test))]
use std::time::Instant;

/// A packet that has not been acked yet
pub struct UnackedMessage {
    message: Message,
    /// If None: this packet has never been sent before
    /// else: the last instant when this packet was sent
    last_sent: Option<Instant>,
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

    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<Message>,

    //
    current_rtt_millis: f32,
    current_time: Instant,
}

impl ReliableSender {
    pub fn new(reliable_settings: ReliableSettings) -> Self {
        Self {
            reliable_settings,
            unacked_messages: Default::default(),
            next_send_message_id: MessageId(0),
            messages_to_send: Default::default(),
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

    /// Internal helper to collect the list of messages that need to be sent
    /// Either because they have never been sent, or because they need to be resent
    fn collect_messages_to_send(&mut self) {
        // resend delay is based on the rtt
        let resend_delay = Duration::from_millis(
            (self.reliable_settings.rtt_resend_factor * self.current_rtt_millis) as u64,
        );

        // Iterate through all unacked messages, oldest message ids first
        for (message_id, message) in self.unacked_messages.iter_mut() {
            let should_send = match message.last_sent {
                // send it the message has never been sent
                None => true,
                // or if we sent it a while back but didn't get an ack
                Some(last_sent) => self.current_time - last_sent > resend_delay,
            };
            if should_send {
                message.message.id = Some(*message_id);
                // TODO: avoid this clone!
                self.messages_to_send.push_back(message.message.clone());
                message.last_sent = Some(self.current_time);
            }
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
    fn buffer_send(&mut self, message: Message) {
        let unacked_message = UnackedMessage {
            message,
            last_sent: None,
        };
        self.unacked_messages
            .insert(self.next_send_message_id, unacked_message);
        self.next_send_message_id += 1;
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self, packet_manager: &mut PacketManager) -> Vec<Packet> {
        // TODO: do we want to ALWAYS call this when we send packet? or should we separate the 2?
        // collect the messages that need to be sent
        // (notably unacked messages that need to be resent)
        self.collect_messages_to_send();

        // build the packets from those messages
        MessagePacker::pack_messages(&mut self.messages_to_send, packet_manager)
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelSend;
    use super::Instant;
    use super::ReliableSender;
    use super::{Message, MessageId};
    use crate::channel::channel::ReliableSettings;
    use crate::channel::receivers::ordered_reliable::OrderedReliableReceiver;
    use bytes::Bytes;
    use mock_instant::MockClock;
    use std::time::Duration;

    #[test]
    fn test_reliable_sender_internals() {
        let mut sender = ReliableSender {
            reliable_settings: ReliableSettings {
                rtt_resend_factor: 1.5,
            },
            unacked_messages: Default::default(),
            next_send_message_id: MessageId(0),
            messages_to_send: Default::default(),
            current_rtt_millis: 100.0,
            current_time: Instant::now(),
        };

        // Buffer a new message
        let mut message = Message::new(Bytes::from("hello"));
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
        assert_eq!(sender.messages_to_send.len(), 2);
        assert_eq!(
            sender.messages_to_send.get(0).unwrap(),
            &(Some(MessageId(0)), message.clone())
        );
        assert_eq!(
            sender.messages_to_send.get(1).unwrap(),
            &(Some(MessageId(0)), message.clone())
        );

        // Ack the first message
        sender.process_message_ack(MessageId(0));
        assert_eq!(sender.unacked_messages.len(), 0);
        assert!(sender.unacked_messages.get(&MessageId(0)).is_none());

        // Advance by a time that is above the resend threshold
        MockClock::advance(Duration::from_millis(200));
        sender.current_time = Instant::now();
        // this time there are no new messages to send
        assert_eq!(sender.messages_to_send.len(), 2);
    }
}
