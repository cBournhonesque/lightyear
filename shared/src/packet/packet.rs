use crate::packet::header::{PacketHeader, PacketHeaderManager};
use crate::packet::message::Message;
use crate::packet::packet_type::PacketType;
use crate::packet::wrapping_id::MessageId;
use anyhow::anyhow;
use bytes::Bytes;

pub trait PacketData {}

/// Single individual packet sent over the network
/// Contains multiple small messages
pub struct SinglePacket {
    header: PacketHeader,
    data: Vec<(MessageId, Message)>,
}

/// A packet that is split into multiple fragments
/// because it contains a message that is too big
pub struct FragmentedPacket {}

/// Abstraction for data that is sent over the network
///
/// Every packet knows how to serialize itself into a list of Single Packets that can
/// directly be sent through a Socket
pub enum Packet {
    Single(SinglePacket),
    Fragmented(FragmentedPacket),
}

impl Packet {
    pub fn messages_in_packet(&self) -> Vec<MessageId> {
        unimplemented!()
    }

    pub fn serialize(&self) -> Bytes {
        unimplemented!()
    }
}

/// Helper to write a packet
pub struct PacketWriter {
    /// Maximum size that the payload of a packet can take
    capacity_bits: u16,
    /// Manage writing packet headers
    packet_header_manager: PacketHeaderManager,
}

impl PacketWriter {
    /// Returns true if the given number of bits can fit into the packet
    pub fn can_fit(&self, num_bits: u32) -> bool {
        unimplemented!()
    }

    /// Start building new packet
    pub fn build_new_packet(&mut self) -> Packet {
        Packet::Single(SinglePacket {
            // TODO: handle protocol and packet type
            header: self
                .packet_header_manager
                .prepare_send_packet_header(0, PacketType::Data),
            data: vec![],
        })
    }

    pub fn try_add_message(
        &mut self,
        packet: &mut Packet,
        message_id: MessageId,
        message: Message,
    ) -> anyhow::Result<()> {
        match packet {
            Packet::Single(single_packet) => {
                if self.can_fit(message.bit_len()) {
                    single_packet.data.push((message_id, message));
                    Ok(())
                } else {
                    Err(anyhow!("Message too big to fit in packet"))
                }
            }
            _ => unimplemented!(),
        }
    }
}
