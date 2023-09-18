use crate::packet::manager::PacketManager;
use crate::packet::message::Message;
use crate::packet::packet::Packet;
use enum_dispatch::enum_dispatch;

pub(crate) mod message_packer;
pub(crate) mod reliable;
pub(crate) mod unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelSend: Send + Sync {
    /// Queues a message to be transmitted
    fn buffer_send(&mut self, message: Message);

    /// Reads from the buffer of message to send to prepare a list of Packets
    /// that can be sent over the network
    fn send_packet(&mut self, packet_manager: &mut PacketManager) -> Vec<Packet>;
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[enum_dispatch(ChannelSend)]
pub enum ChannelSender {
    UnorderedUnreliable(unreliable::UnorderedUnreliableSender),
    SequencedUnreliable(unreliable::SequencedUnreliableSender),
    Reliable(reliable::ReliableSender),
}
