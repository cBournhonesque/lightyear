use std::collections::{BTreeMap, HashMap};

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::packet::header::PacketHeader;
use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet_type::PacketType;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;

pub trait PacketData {}

pub(crate) const MTU_PACKET_BYTES: usize = 1250;
const HEADER_BYTES: usize = 50;
/// The maximum of bytes that the payload of the packet can contain (excluding the header)
pub(crate) const MTU_PAYLOAD_BYTES: usize = 1200;

/// Single individual packet sent over the network
/// Contains multiple small messages
#[derive(Clone)]
pub(crate) struct SinglePacket<P: BitSerializable, const C: usize = MTU_PACKET_BYTES> {
    // TODO: the alternative is to simply encode the bytes in the single packet
    //  the packetManager has all the necessary information to know how to encode
    //  a list of messages tied to a channel into a packet, and to decode them
    //  PROS: all complexity is only in one object (packet_manager), less indirection
    //  CONS: less clear what the structure of a packet is. Hidden knowledge inside PacketManager
    // pub(crate) bytes: [u8; MTU_PACKET_BYTES],

    // TODO: the alternative is to encode the structure of the packet in the struct
    //  and have custom encode/decode functions for it that take care of continue bits, etc.
    //  data: BTreeMap<NetId, Vec<MessageContainer<P>>>,
    //  PROS: less "magic" in the packet_manager, more explicit of how a packet is built
    //  CONS: more boilerplate, more complicated?
    pub(crate) header: PacketHeader,
    pub(crate) data: BTreeMap<NetId, Vec<MessageContainer<P>>>,
}

impl<P: BitSerializable> SinglePacket<P> {
    fn new(packet_manager: &mut PacketManager<P>) -> Self {
        Self {
            header: packet_manager
                .header_manager
                .prepare_send_packet_header(0, PacketType::Data),
            // bytes: [0; MTU_PACKET_BYTES],
            data: Default::default(),
        }
    }

    pub fn add_channel(&mut self, channel: NetId) {
        self.data.entry(channel).or_insert(Vec::new());
    }

    pub fn add_message(&mut self, channel: NetId, message: MessageContainer<P>) {
        self.data.entry(channel).or_insert(Vec::new()).push(message);
    }

    /// Return the list of message ids in the packet
    pub fn message_ids(&self) -> HashMap<NetId, Vec<MessageId>> {
        self.data
            .iter()
            .map(|(&net_id, messages)| {
                let message_ids: Vec<MessageId> =
                    messages.iter().filter_map(|message| message.id).collect();
                (net_id, message_ids)
            })
            .collect()
    }

    #[cfg(test)]
    pub fn num_messages(&self) -> usize {
        self.data.iter().map(|(_, messages)| messages.len()).sum()
    }
}

impl<P: BitSerializable> BitSerializable for SinglePacket<P> {
    /// An expectation of the encoding is that we always have at least one channel that we can encode per packet.
    /// However, some channels might not have any messages (for example if we start writing the channel at the very end of the packet)
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.serialize(&self.header)?;

        self.data
            .iter()
            .enumerate()
            .map(|(i, v)| (i == self.data.len() - 1, v))
            .map(|(is_last_channel, (channel_id, messages))| {
                writer.serialize(channel_id)?;

                // initial continue bit for messages (are there messages for this channel or not?)
                writer.serialize(&!messages.is_empty())?;
                messages
                    .iter()
                    .enumerate()
                    .map(|(j, w)| (j == messages.len() - 1, w))
                    .map(|(is_last_message, message)| {
                        message.encode(writer)?;
                        // write message continue bit (1 if there is another message to writer after)
                        writer.serialize(&!is_last_message)?;
                        Ok::<(), anyhow::Error>(())
                    })
                    .collect::<anyhow::Result<()>>()?;
                // write channel continue bit (1 if there is another channel to writer after)
                writer.serialize(&!is_last_channel)?;
                Ok::<(), anyhow::Error>(())
            })
            .collect::<anyhow::Result<()>>()?;
        Ok(())
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let header = reader.deserialize::<PacketHeader>()?;

        let mut data = BTreeMap::new();
        let mut continue_read_channel = true;

        // check channel continue bit to see if there are more channels
        while continue_read_channel {
            let mut channel_id = reader.deserialize::<NetId>()?;

            let mut messages = Vec::new();

            // are there messages for this channel?
            let mut continue_read_message = reader.deserialize::<bool>()?;
            // check message continue bit to see if there are more messages
            while continue_read_message {
                let message = <MessageContainer<P>>::decode(reader)?;
                messages.push(message);
                continue_read_message = reader.deserialize::<bool>()?;
            }
            data.insert(channel_id, messages);
            continue_read_channel = reader.deserialize::<bool>()?;
        }
        Ok(Self { header, data })
    }
}

/// A packet that is split into multiple fragments
/// because it contains a message that is too big
pub struct FragmentedPacket {}

/// Abstraction for data that is sent over the network
///
/// Every packet knows how to serialize itself into a list of Single Packets that can
/// directly be sent through a Socket
pub(crate) enum Packet<P: BitSerializable> {
    Single(SinglePacket<P>),
    Fragmented(FragmentedPacket),
}

impl<P: BitSerializable> Packet<P> {
    pub(crate) fn new(packet_manager: &mut PacketManager<P>) -> Self {
        Packet::Single(SinglePacket::new(packet_manager))
    }

    /// Encode a packet into the write buffer
    pub fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        match self {
            Packet::Single(single_packet) => single_packet.encode(writer),
            _ => unimplemented!(),
        }
    }

    /// Decode a packet from the read buffer. The read buffer will only contain the bytes for a single packet
    pub fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Packet<P>> {
        let single_packet = SinglePacket::<P>::decode(reader)?;
        Ok(Packet::Single(single_packet))
    }

    // #[cfg(test)]
    pub fn header(&self) -> &PacketHeader {
        match self {
            Packet::Single(single_packet) => &single_packet.header,
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    pub fn add_channel(&mut self, channel: NetId) {
        match self {
            Packet::Single(single_packet) => {
                single_packet.add_channel(channel);
            }
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    pub fn add_message(&mut self, channel: NetId, message: MessageContainer<P>) -> () {
        match self {
            Packet::Single(single_packet) => {
                single_packet.add_message(channel, message);
            }
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    /// Number of messages currently written in the packet
    #[cfg(test)]
    pub fn num_messages(&self) -> usize {
        match self {
            Packet::Single(single_packet) => single_packet.num_messages(),
            Packet::Fragmented(_fragmented_packet) => unimplemented!(),
        }
    }

    /// Return the list of messages in the packet
    pub fn message_ids(&self) -> HashMap<NetId, Vec<MessageId>> {
        match self {
            Packet::Single(single_packet) => single_packet.message_ids(),
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
}

#[cfg(test)]
mod tests {
    use lightyear_derive::ChannelInternal;

    use crate::packet::manager::PacketManager;
    use crate::packet::packet::{Packet, SinglePacket};
    use crate::packet::packet_type::PacketType;
    use crate::{
        BitSerializable, ChannelDirection, ChannelMode, ChannelRegistry, ChannelSettings,
        MessageContainer, ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer,
    };

    #[derive(ChannelInternal)]
    struct Channel1;

    #[derive(ChannelInternal)]
    struct Channel2;

    fn get_channel_registry() -> ChannelRegistry {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        };
        let mut c = ChannelRegistry::new();
        c.add::<Channel1>(settings.clone());
        c.add::<Channel2>(settings.clone());
        c
    }

    #[test]
    fn test_single_packet_add_messages() {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::<i32>::new(channel_registry.kind_map());
        let mut packet = SinglePacket::new(&mut manager);

        packet.add_message(0, MessageContainer::new(0));
        packet.add_message(0, MessageContainer::new(1));
        packet.add_message(1, MessageContainer::new(2));

        assert_eq!(packet.num_messages(), 3);
    }

    #[test]
    fn test_encode_single_packet() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::<i32>::new(channel_registry.kind_map());
        let mut packet = Packet::new(&mut manager);

        let mut write_buffer = WriteWordBuffer::with_capacity(50);
        packet.add_message(0, MessageContainer::new(0));
        packet.add_message(0, MessageContainer::new(1));
        packet.add_message(1, MessageContainer::new(2));
        // add a channel with no messages
        packet.add_channel(2);

        packet.encode(&mut write_buffer);
        let packet_bytes = write_buffer.finish_write();

        // Encode manually
        let mut expected_write_buffer = WriteWordBuffer::with_capacity(50);
        expected_write_buffer.serialize(packet.header())?;
        // channel id
        expected_write_buffer.serialize(&0u16)?;
        // messages, with continuation bit
        expected_write_buffer.serialize(&true)?;
        MessageContainer::new(0).encode(&mut expected_write_buffer)?;
        expected_write_buffer.serialize(&true)?;
        MessageContainer::new(1).encode(&mut expected_write_buffer)?;
        expected_write_buffer.serialize(&false)?;
        // channel continue bit
        expected_write_buffer.serialize(&true)?;
        // channel id
        expected_write_buffer.serialize(&1u16)?;
        // messages with continuation bit
        expected_write_buffer.serialize(&true)?;
        MessageContainer::new(2).encode(&mut expected_write_buffer)?;
        expected_write_buffer.serialize(&false)?;
        // channel continue bit
        expected_write_buffer.serialize(&true)?;
        // channel id
        expected_write_buffer.serialize(&2u16)?;
        // messages with continuation bit
        expected_write_buffer.serialize(&false)?;
        // channel continue bit
        expected_write_buffer.serialize(&false)?;

        let expected_packet_bytes = expected_write_buffer.finish_write();

        assert_eq!(packet_bytes, expected_packet_bytes);

        let mut reader = ReadWordBuffer::start_read(packet_bytes);
        let packet = SinglePacket::<i32>::decode(&mut reader)?;

        assert_eq!(packet.header.get_packet_type(), PacketType::Data);
        assert_eq!(packet.num_messages(), 3);
        Ok(())
    }
}
