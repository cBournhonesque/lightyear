use crate::packet::header::PacketHeader;
use crate::packet::message::Message;
use crate::packet::wrapping_id::MessageId;
use bitcode::{Decode, Encode};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub trait PacketData {}

const HEADER_BYTES: usize = 50;
/// The maximum of bytes that the payload of the packet can contain (excluding the header)
pub(crate) const MTU_PACKET_BYTES: usize = 1200;

/// Single individual packet sent over the network
/// Contains multiple small messages
#[derive(Encode, Decode, Serialize, Deserialize)]
pub(crate) struct SinglePacket {
    pub(crate) header: PacketHeader,
    pub(crate) data: Vec<Message>,
}

// impl SinglePacket {
//     /// Serialized bytes for a single packet
//     pub fn serialize(&self) -> anyhow::Result<Bytes> {
//         // allocate some bits for encoding
//         let mut encode_buf = bitcode::Buffer::with_capacity(HEADER_BYTES);
//         let header_bytes = encode_buf.encode(&self.header)?;
//         let mut bytes = bytes::BytesMut::with_capacity(MTU_PACKET_BYTES);
//         bytes.extend(header_bytes);
//         for message in &self.data {
//             let message_bytes = message.to_bytes()?;
//             bytes.extend(message_bytes);
//         }
//         Ok(bytes.freeze())
//     }
//
//     pub fn deserialize(bytes: &[u8]) -> anyhow::Result<Self> {
//         unimplemented!()
//     }
// }

/// A packet that is split into multiple fragments
/// because it contains a message that is too big
#[derive(Encode, Decode, Serialize, Deserialize)]
pub struct FragmentedPacket {}

/// Abstraction for data that is sent over the network
///
/// Every packet knows how to serialize itself into a list of Single Packets that can
/// directly be sent through a Socket
#[derive(Encode, Decode, Serialize, Deserialize)]
pub enum Packet {
    Single(SinglePacket),
    Fragmented(FragmentedPacket),
}

impl Packet {
    #[cfg(test)]
    pub fn header(&self) -> &PacketHeader {
        match self {
            Packet::Single(single_packet) => &single_packet.header,
            Packet::Fragmented(fragmented_packet) => unimplemented!(),
        }
    }

    /// Number of messages currently written in the packet
    #[cfg(test)]
    pub fn num_messages(&self) -> usize {
        match self {
            Packet::Single(single_packet) => single_packet.data.len(),
            Packet::Fragmented(fragmented_packet) => unimplemented!(),
        }
    }

    /// Return the list of messages in the packet
    pub fn messages(&self) -> Vec<&Message> {
        match self {
            Packet::Single(single_packet) => single_packet.data.iter().collect(),
            Packet::Fragmented(fragmented_packet) => unimplemented!(),
        }
    }

    /// Construct the list of single packets to be sent over the network from this packet
    pub fn split(self) -> Vec<SinglePacket> {
        match self {
            Packet::Single(single_packet) => vec![single_packet],
            Packet::Fragmented(fragmented_packet) => unimplemented!(),
        }
    }

    /// Contains the list of message ids that are in this packet
    pub fn message_ids(&self) -> Vec<MessageId> {
        match self {
            Packet::Single(single_packet) => single_packet
                .data
                .iter()
                .filter_map(|message| message.id)
                .collect(),
            Packet::Fragmented(fragmented_packet) => unimplemented!(),
        }
    }
}
