use std::collections::VecDeque;

use crate::channel::senders::ChannelSend;
use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::BitSerializable;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
pub struct UnorderedUnreliableSender<P> {
    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<MessageContainer<P>>,
}

impl<P> UnorderedUnreliableSender<P> {
    pub(crate) fn new() -> Self {
        Self {
            messages_to_send: VecDeque::new(),
        }
    }
}

impl<P: BitSerializable> ChannelSend<P> for UnorderedUnreliableSender<P> {
    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: MessageContainer<P>) {
        self.messages_to_send.push_back(message);
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self, packet_manager: &mut PacketManager<P>) {
        let messages_to_send = std::mem::take(&mut self.messages_to_send);
        let (remaining_messages_to_send, _) =
            packet_manager.pack_messages_within_channel(messages_to_send);
        self.messages_to_send = remaining_messages_to_send;
    }

    // not necessary for an unreliable sender (all the buffered messages can be sent)
    fn collect_messages_to_send(&mut self) {}

    fn notify_message_delivered(&mut self, message_id: &MessageId) {}

    fn has_messages_to_send(&self) -> bool {
        !self.messages_to_send.is_empty()
    }
}

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information
pub struct SequencedUnreliableSender<P> {
    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<MessageContainer<P>>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
}

impl<P> SequencedUnreliableSender<P> {
    pub(crate) fn new() -> Self {
        Self {
            messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId(0),
        }
    }
}

impl<P: BitSerializable> ChannelSend<P> for SequencedUnreliableSender<P> {
    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, mut message: MessageContainer<P>) {
        message.id = Some(self.next_send_message_id);
        self.messages_to_send.push_back(message);
        self.next_send_message_id += 1;
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self, packet_manager: &mut PacketManager<P>) {
        let messages_to_send = std::mem::take(&mut self.messages_to_send);
        let (remaining_messages_to_send, _) =
            packet_manager.pack_messages_within_channel(messages_to_send);
        self.messages_to_send = remaining_messages_to_send;
    }

    // not necessary for an unreliable sender (all the buffered messages can be sent)
    fn collect_messages_to_send(&mut self) {}

    fn notify_message_delivered(&mut self, message_id: &MessageId) {}

    fn has_messages_to_send(&self) -> bool {
        !self.messages_to_send.is_empty()
    }
}
