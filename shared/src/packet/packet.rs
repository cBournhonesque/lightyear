use std::collections::{BTreeMap, HashMap};

use bitcode::{Decode, Encode};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::packet::header::PacketHeader;
use crate::packet::message::{MessageContainer, SingleData};
use crate::packet::packet_manager::PacketManager;
use crate::packet::packet_type::PacketType;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;

pub(crate) const MTU_PACKET_BYTES: usize = 1250;
const HEADER_BYTES: usize = 50;
/// The maximum of bytes that the payload of the packet can contain (excluding the header)
pub(crate) const MTU_PAYLOAD_BYTES: usize = 1200;
pub(crate) const FRAGMENT_SIZE: usize = 1200;

// TODO: we don't need SinglePacket vs FragmentPacket; we can just re-use the same thing
//  because MessageContainer already has the information about whether it is a fragment or not
//  we just have an underlying assumption that in a fragment packet, the first message will be a fragment message,
//  and all others will be normal messages
//  The reason we do this is we dont want to pay 1 bit on every message to know if it's fragmented or not

// pub(crate) struct Packet<const C: usize = MTU_PACKET_BYTES> {
//     pub(crate) data: BTreeMap<NetId, Vec<MessageContainer>>
// }

/// Single individual packet sent over the network
/// Contains multiple small messages
#[derive(Clone, Debug)]
pub(crate) struct SinglePacket<const C: usize = MTU_PACKET_BYTES> {
    pub(crate) data: BTreeMap<NetId, Vec<SingleData>>,
}

impl SinglePacket {
    pub(crate) fn new() -> Self {
        Self {
            data: Default::default(),
        }
    }

    pub fn add_channel(&mut self, channel: NetId) {
        self.data.entry(channel).or_default();
    }

    pub fn add_message(&mut self, channel: NetId, message: SingleData) {
        self.data.entry(channel).or_default().push(message);
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

impl BitSerializable for SinglePacket {
    /// An expectation of the encoding is that we always have at least one channel that we can encode per packet.
    /// However, some channels might not have any messages (for example if we start writing the channel at the very end of the packet)
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        self.data
            .iter()
            .enumerate()
            .map(|(i, v)| (i == self.data.len() - 1, v))
            .try_for_each(|(is_last_channel, (channel_id, messages))| {
                writer.serialize(channel_id)?;

                // initial continue bit for messages (are there messages for this channel or not?)
                writer.serialize(&!messages.is_empty())?;
                messages
                    .iter()
                    .enumerate()
                    .map(|(j, w)| (j == messages.len() - 1, w))
                    .try_for_each(|(is_last_message, message)| {
                        message.encode(writer)?;
                        // write message continue bit (1 if there is another message to writer after)
                        writer.serialize(&!is_last_message)?;
                        Ok::<(), anyhow::Error>(())
                    })?;
                // write channel continue bit (1 if there is another channel to writer after)
                writer.serialize(&!is_last_channel)?;
                Ok::<(), anyhow::Error>(())
            })
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
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
                // TODO: we use decode here because we write Bytes directly (we already serialized from the object to Bytes)
                //  Could we do a memcpy instead?
                let message = reader.decode::<SingleData>()?;
                // let message = <SingleData>::decode(reader)?;
                messages.push(message);
                continue_read_message = reader.deserialize::<bool>()?;
            }
            data.insert(channel_id, messages);
            continue_read_channel = reader.deserialize::<bool>()?;
        }
        Ok(Self { data })
    }
}

/// A packet that is split into multiple fragments
/// because it contains a message that is too big
#[derive(Clone, Debug)]
pub struct FragmentedPacket {
    pub(crate) channel_id: NetId,
    pub(crate) fragment: FragmentData,
    // TODO: change this as option? only the last fragment might have this
    /// Normal packet data: header + eventual non-fragmented messages included in the packet
    pub(crate) packet: SinglePacket,
}

#[derive(Clone, Debug)]
pub struct FragmentData {
    // we always need a message_id for fragment messages, for re-assembly
    pub message_id: MessageId,
    pub fragment_id: u8,
    pub num_fragments: u8,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
}

impl FragmentData {
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.serialize(&self.message_id)?;
        writer.serialize(&self.fragment_id)?;
        writer.serialize(&self.num_fragments)?;
        // TODO: be able to just concat the bytes to the buffer?
        if self.is_last_fragment() {
            /// writing the slice includes writing the length of the slice
            writer.serialize(self.bytes.as_ref());
            // writer.serialize(&self.bytes.to_vec());
            // writer.serialize(&self.fragment_message_bytes.as_ref());
        } else {
            let bytes_array: [u8; FRAGMENT_SIZE] = self.bytes.as_ref().try_into().unwrap();
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            writer.encode(&bytes_array);
        }
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let message_id = reader.deserialize::<MessageId>()?;
        let fragment_id = reader.deserialize::<u8>()?;
        let num_fragments = reader.deserialize::<u8>()?;
        let mut bytes: Bytes;
        if fragment_id == num_fragments - 1 {
            let read_bytes = reader.deserialize::<Vec<u8>>()?;
            bytes = Bytes::from(read_bytes);
        } else {
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            let read_bytes = reader.decode::<[u8; FRAGMENT_SIZE]>()?;
            let bytes_vec: Vec<u8> = read_bytes.to_vec();
            bytes = Bytes::from(bytes_vec);
        }
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            bytes,
        })
    }

    pub(crate) fn is_last_fragment(&self) -> bool {
        self.fragment_id == self.num_fragments - 1
    }

    fn num_fragment_bytes(&self) -> usize {
        if self.is_last_fragment() {
            self.bytes.len()
        } else {
            FRAGMENT_SIZE
        }
    }
}

impl FragmentedPacket {
    pub(crate) fn new(channel_id: NetId, fragment: FragmentData) -> Self {
        Self {
            channel_id,
            fragment,
            packet: SinglePacket::new(),
        }
    }
}

impl BitSerializable for FragmentedPacket {
    /// An expectation of the encoding is that we always have at least one channel that we can encode per packet.
    /// However, some channels might not have any messages (for example if we start writing the channel at the very end of the packet)
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.serialize(&self.channel_id)?;
        self.fragment.encode(writer)?;
        self.packet.encode(writer)
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let mut channel_id = reader.deserialize::<NetId>()?;
        let fragment = FragmentData::decode(reader)?;
        let packet = SinglePacket::decode(reader)?;
        Ok(Self {
            channel_id,
            fragment,
            packet,
        })
    }
}

/// Abstraction for data that is sent over the network
///
/// Every packet knows how to serialize itself into a list of Single Packets that can
/// directly be sent through a Socket
#[cfg_attr(feature = "debug", derive(Debug))]
pub(crate) enum PacketData {
    Single(SinglePacket),
    Fragmented(FragmentedPacket),
}

impl PacketData {
    pub(crate) fn contents(self) -> HashMap<NetId, Vec<MessageContainer>> {
        let mut res = HashMap::new();
        match self {
            PacketData::Single(data) => {
                for (channel_id, messages) in data.data {
                    res.insert(
                        channel_id,
                        messages.into_iter().map(|data| data.into()).collect(),
                    );
                }
            }
            PacketData::Fragmented(data) => {
                // add fragment
                res.insert(data.channel_id, vec![data.fragment.into()]);
                // add other single messages
                for (channel_id, messages) in data.packet.data {
                    res.insert(
                        channel_id,
                        messages.into_iter().map(|data| data.into()).collect(),
                    );
                }
            }
        }
        res
    }
}

pub(crate) struct Packet {
    pub(crate) header: PacketHeader,
    pub(crate) data: PacketData,
}

impl Packet {
    pub(crate) fn is_empty(&self) -> bool {
        match &self.data {
            PacketData::Single(single_packet) => single_packet.data.is_empty(),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.packet.data.is_empty(),
        }
    }

    /// Encode a packet into the write buffer
    pub fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        match &self.data {
            PacketData::Single(single_packet) => single_packet.encode(writer),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.encode(writer),
        }
    }

    /// Decode a packet from the read buffer. The read buffer will only contain the bytes for a single packet
    pub fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Packet> {
        let header = PacketHeader::decode(reader)?;
        let packet_type = header.get_packet_type();
        match packet_type {
            PacketType::Data => {
                let single_packet = SinglePacket::decode(reader)?;
                Ok(Self {
                    header,
                    data: PacketData::Single(single_packet),
                })
            }
            PacketType::DataFragment => {
                let fragmented_packet = FragmentedPacket::decode(reader)?;
                Ok(Self {
                    header,
                    data: PacketData::Fragmented(fragmented_packet),
                })
            }
            _ => Err(anyhow::anyhow!("Packet type not supported")),
        }
    }

    // #[cfg(test)]
    pub fn header(&self) -> &PacketHeader {
        &self.header
    }

    pub fn add_channel(&mut self, channel: NetId) {
        match &mut self.data {
            PacketData::Single(single_packet) => {
                single_packet.add_channel(channel);
            }
            PacketData::Fragmented(fragmented_packet) => {
                fragmented_packet.packet.add_channel(channel);
            }
        }
    }

    pub fn add_message(&mut self, channel: NetId, message: SingleData) {
        match &mut self.data {
            PacketData::Single(single_packet) => single_packet.add_message(channel, message),
            PacketData::Fragmented(fragmented_packet) => {
                fragmented_packet.packet.add_message(channel, message);
            }
        }
    }

    /// Number of messages currently written in the packet
    #[cfg(test)]
    pub fn num_messages(&self) -> usize {
        match &self.data {
            PacketData::Single(single_packet) => single_packet.num_messages(),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.packet.num_messages(),
        }
    }

    /// Return the list of messages in the packet
    pub fn message_ids(&self) -> HashMap<NetId, Vec<MessageId>> {
        match &self.data {
            PacketData::Single(single_packet) => single_packet.message_ids(),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.packet.message_ids(),
        }
    }
}

#[cfg(test)]
mod tests {
    use lightyear_derive::ChannelInternal;

    use crate::packet::packet::{PacketData, SinglePacket};
    use crate::packet::packet_manager::PacketManager;
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
        let mut packet = PacketData::new_single(&mut manager);

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
