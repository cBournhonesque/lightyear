use crate::channel::senders::message_packer::MessagePacker;
use crate::channel::senders::ChannelSend;
use crate::packet::manager::PacketManager;
use crate::packet::message::Message;
use crate::packet::packet::Packet;
use crate::packet::wrapping_id::MessageId;
use std::collections::VecDeque;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
pub struct UnorderedUnreliableSender {
    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<Message>,
}

impl UnorderedUnreliableSender {
    pub(crate) fn new() -> Self {
        Self {
            messages_to_send: VecDeque::new(),
        }
    }
}

impl ChannelSend for UnorderedUnreliableSender {
    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, message: Message) {
        self.messages_to_send.push_back(message);
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self, packet_manager: &mut PacketManager) -> Vec<Packet> {
        MessagePacker::pack_messages(&mut self.messages_to_send, packet_manager)
    }
}

/// A sender that simply sends the messages without checking if they were received
/// Same as UnorderedUnreliableSender, but includes ordering information
pub struct SequencedUnreliableSender {
    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<Message>,
    /// Message id to use for the next message to be sent
    next_send_message_id: MessageId,
}

impl SequencedUnreliableSender {
    pub(crate) fn new() -> Self {
        Self {
            messages_to_send: VecDeque::new(),
            next_send_message_id: MessageId(0),
        }
    }
}

impl ChannelSend for SequencedUnreliableSender {
    /// Add a new message to the buffer of messages to be sent.
    /// This is a client-facing function, to be called when you want to send a message
    fn buffer_send(&mut self, mut message: Message) {
        message.id = Some(self.next_send_message_id);
        self.messages_to_send.push_back(message);
        self.next_send_message_id += 1;
    }

    /// Take messages from the buffer of messages to be sent, and build a list of packets
    /// to be sent
    fn send_packet(&mut self, packet_manager: &mut PacketManager) -> Vec<Packet> {
        MessagePacker::pack_messages(&mut self.messages_to_send, packet_manager)
    }
}
