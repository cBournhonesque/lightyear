use crate::packet::message::Message;
use crate::packet::packet::{Packet, PacketWriter};

pub(crate) mod message_packer;
pub(crate) mod reliable;
pub(crate) mod unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
pub trait ChannelSender: Send + Sync {
    /// Queues a message to be transmitted
    fn buffer_send(&mut self, message: Message);

    /// Reads from the buffer of message to send to prepare a list of Packets
    /// that can be sent over the network
    fn send_packet(&mut self, packet_writer: &mut PacketWriter) -> Vec<Packet>;
}
