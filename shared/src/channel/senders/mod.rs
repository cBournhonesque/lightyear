use enum_dispatch::enum_dispatch;

use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::Packet;

pub(crate) mod message_packer;
pub(crate) mod reliable;
pub(crate) mod unreliable;

/// A trait for sending messages to a channel.
/// A channel is a buffer over packets to be able to add ordering/reliability
#[enum_dispatch]
pub trait ChannelSend<P> {
    /// Queues a message to be transmitted
    fn buffer_send(&mut self, message: MessageContainer<P>);

    /// Reads from the buffer of message to send to prepare a list of Packets
    /// that can be sent over the network
    fn send_packet(&mut self, packet_manager: &mut PacketManager<P>) -> Vec<Packet<P>>;
}

/// Enum dispatch lets us derive ChannelSend on each enum variant
#[enum_dispatch(ChannelSend)]
pub enum ChannelSender<P> {
    UnorderedUnreliable(unreliable::UnorderedUnreliableSender<P>),
    SequencedUnreliable(unreliable::SequencedUnreliableSender<P>),
    Reliable(reliable::ReliableSender<P>),
}
