use crate::channel::senders::message_packer::MessagePacker;
use crate::channel::senders::ChannelSender;
use crate::packet::message::Message;
use crate::packet::packet::{Packet, PacketWriter};
use std::collections::VecDeque;

/// A sender that simply sends the messages without checking if they were received
/// Does not include any ordering information
pub struct UnorderedUnreliableSender {
    /// list of messages that we want to fit into packets and send
    messages_to_send: VecDeque<Message>,
}

// impl ChannelSender for UnorderedUnreliableSender {
//     /// Add a new message to the buffer of messages to be sent.
//     /// This is a client-facing function, to be called when you want to send a message
//     fn buffer_send(&mut self, message: Message) {
//         self.messages_to_send.push_back(message);
//     }
//
//     /// Take messages from the buffer of messages to be sent, and build a list of packets
//     /// to be sent
//     fn send_packet(&mut self, packet_writer: &mut PacketWriter) -> Vec<Packet> {
//         MessagePacker::pack_messages(&mut self.messages_to_send, packet_writer)
//         // MessagePacker::pack_messages(&mut self.messages_to_send, packet_writer)
//     }
// }
