use bitcode::{Decode, Encode};
use serde::Serialize;

use crate::packet::header::PacketHeader;
use crate::packet::message::MessageContainer;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::SerializableProtocol;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;

pub trait PacketData {}

const HEADER_BYTES: usize = 50;
/// The maximum of bytes that the payload of the packet can contain (excluding the header)
pub(crate) const MTU_PACKET_BYTES: usize = 1200;

/// Single individual packet sent over the network
/// Contains multiple small messages
pub(crate) struct SinglePacket<P> {
    pub(crate) header: PacketHeader,
    pub(crate) data: Vec<MessageContainer<P>>,
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
pub struct FragmentedPacket {}

/// Abstraction for data that is sent over the network
///
/// Every packet knows how to serialize itself into a list of Single Packets that can
/// directly be sent through a Socket
pub(crate) enum Packet<P> {
    Single(SinglePacket<P>),
    Fragmented(FragmentedPacket),
}

impl<P: SerializableProtocol> Packet<P> {
    /// Encode a packet into the write buffer
    pub fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        match self {
            Packet::Single(single_packet) => {
                writer.serialize(&single_packet.header)?;
                // TODO: include channel information. Maybe in the header?

                // TODO: does question mark work inside an iterator?
                single_packet.data.iter().try_for_each(|message| {
                    message.encode(writer)?;
                    Ok(())
                })
            }
            _ => unimplemented!(),
        }
    }

    /// Decode a packet from the read buffer. The read buffer will only contain the bytes for a single packet
    pub fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Packet<P>> {
        let header = reader.deserialize()?;
        // TODO: get channel information
        let mut data = Vec::new();
        // TODO: add info about num messages in packet?
        // TODO: loop over messages
        let message = MessageContainer::decode(reader)?;
        data.push(message);
        Ok(Packet::Single(SinglePacket { header, data }))
    }

    #[cfg(test)]
    pub fn header(&self) -> &PacketHeader {
        match self {
            Packet::Single(single_packet) => &single_packet.header,
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    pub fn add_message(&mut self, message: MessageContainer<P>) -> () {
        match self {
            Packet::Single(single_packet) => single_packet.data.push(message),
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    /// Number of messages currently written in the packet
    #[cfg(test)]
    pub fn num_messages(&self) -> usize {
        match self {
            Packet::Single(single_packet) => single_packet.data.len(),
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    /// Return the list of messages in the packet
    pub fn messages(&self) -> Vec<&MessageContainer<P>> {
        match self {
            Packet::Single(single_packet) => single_packet.data.iter().collect(),
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    /// Construct the list of single packets to be sent over the network from this packet
    pub(crate) fn split(self) -> Vec<SinglePacket<P>> {
        match self {
            Packet::Single(single_packet) => vec![single_packet],
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
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
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }
}
