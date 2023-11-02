use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};

use bitcode::__private::Gamma;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentData, MessageAck, MessageContainer, SingleData};
use crate::packet::packet_manager::PacketManager;
use crate::packet::packet_type::PacketType;
use crate::packet::wrapping_id::MessageId;
use crate::protocol::channel::ChannelId;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::{BitRead, ReadBuffer};
use crate::serialize::writer::WriteBuffer;

const HEADER_BYTES: usize = 50;
/// The maximum of bytes that the payload of the packet can contain (excluding the header)
pub(crate) const MTU_PAYLOAD_BYTES: usize = 1150;
// pub(crate) const FRAGMENT_SIZE: usize = 1140;

// TODO: THERE IS SOMETHING WRONG WITH EITHER LAST FRAGMENT OR ALL FRAGMENTS!
pub(crate) const FRAGMENT_SIZE: usize = 500;

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
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SinglePacket {
    pub(crate) data: BTreeMap<NetId, Vec<SingleData>>,
    // num_bits: usize,
}

impl SinglePacket {
    pub(crate) fn new() -> Self {
        Self {
            data: Default::default(),
            // TODO: maybe this should include the header? maybe the header size depends on packet type, so that
            //  normal packets use only 1 bit for the packet type, as an optimization like naia
            // num_bits: 0,
        }
    }

    pub fn add_channel(&mut self, channel: NetId) {
        self.data.entry(channel).or_default();
        // match self.data.entry(channel) {
        //     Entry::Vacant(_) => {}
        //     Entry::Occupied(entry) => {
        //         entry.insert(Vec::new());
        //         // how many bits does the channel id take?
        //         // u16::encode()
        //
        //         // is this approach possible? we need to know what we align.
        //         // do we byte-align every SingleData, it is easier? so that we can copy Bytes more easily?
        //         // self.num_bits += 1;
        //     }
        // }
    }

    pub fn add_message(&mut self, channel: NetId, message: SingleData) {
        self.data.entry(channel).or_default().push(message);
    }

    /// Return the list of message ids in the packet
    pub fn message_acks(&self) -> HashMap<NetId, Vec<MessageAck>> {
        self.data
            .iter()
            .map(|(&net_id, messages)| {
                let message_acks: Vec<MessageAck> = messages
                    .iter()
                    .filter(|message| message.id.is_some())
                    .map(|message| MessageAck {
                        message_id: message.id.unwrap(),
                        fragment_id: None,
                    })
                    .collect();
                (net_id, message_acks)
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
                info!(?channel_id, "ENCODING CHANNEL ID");
                writer.encode(channel_id, Gamma)?;

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
            let mut channel_id = reader.decode::<NetId>(Gamma)?;
            info!(?channel_id, "DECODED CHANNEL ID");

            let mut messages = Vec::new();

            // are there messages for this channel?
            let mut continue_read_message = reader.deserialize::<bool>()?;
            // check message continue bit to see if there are more messages
            while continue_read_message {
                // TODO: we use decode here because we write Bytes directly (we already serialized from the object to Bytes)
                //  Could we do a memcpy instead?
                let message = SingleData::decode(reader)?;
                // let message = reader.decode::<SingleData>(Fixed)?;
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
#[derive(Clone, Debug, PartialEq)]
pub struct FragmentedPacket {
    pub(crate) channel_id: ChannelId,
    pub(crate) fragment: FragmentData,
    // TODO: change this as option? only the last fragment might have this
    /// Normal packet data: header + eventual non-fragmented messages included in the packet
    pub(crate) packet: SinglePacket,
}

impl FragmentedPacket {
    pub(crate) fn new(channel_id: ChannelId, fragment: FragmentData) -> Self {
        Self {
            channel_id,
            fragment,
            packet: SinglePacket::new(),
        }
    }

    /// Return the list of message ids in the packet
    pub(crate) fn message_acks(&self) -> HashMap<ChannelId, Vec<MessageAck>> {
        let mut data: HashMap<_, _> = self
            .packet
            .data
            .iter()
            .map(|(&net_id, messages)| {
                let message_acks: Vec<MessageAck> = messages
                    .iter()
                    .filter(|message| message.id.is_some())
                    .map(|message| MessageAck {
                        message_id: message.id.unwrap(),
                        fragment_id: None,
                    })
                    .collect();
                (net_id, message_acks)
            })
            .collect();
        data.entry(self.channel_id).or_default().push(MessageAck {
            message_id: self.fragment.message_id,
            fragment_id: Some(self.fragment.fragment_id),
        });
        data
    }
}

impl BitSerializable for FragmentedPacket {
    /// An expectation of the encoding is that we always have at least one channel that we can encode per packet.
    /// However, some channels might not have any messages (for example if we start writing the channel at the very end of the packet)
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.encode(&self.channel_id, Gamma)?;
        self.fragment.encode(writer)?;
        self.packet.encode(writer)
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let mut channel_id = reader.decode::<NetId>(Gamma)?;
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
#[derive(Debug)]
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
                // add other single messages (if there are any)
                for (channel_id, messages) in data.packet.data {
                    let message_containers: Vec<MessageContainer> =
                        messages.into_iter().map(|data| data.into()).collect();
                    res.entry(channel_id)
                        .or_default()
                        .extend(message_containers);
                }
            }
        }
        res
    }
}

#[derive(Debug)]
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
        // use encode to force Fixed encoding
        // should still use gamma for packet type
        // TODO: add test
        writer.encode(&self.header, Fixed)?;
        match &self.data {
            PacketData::Single(single_packet) => single_packet.encode(writer),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.encode(writer),
        }
    }

    /// Decode a packet from the read buffer. The read buffer will only contain the bytes for a single packet
    pub fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Packet> {
        let header = reader.decode::<PacketHeader>(Fixed)?;
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

    pub fn message_acks(&self) -> HashMap<ChannelId, Vec<MessageAck>> {
        match &self.data {
            PacketData::Single(single_packet) => single_packet.message_acks(),
            PacketData::Fragmented(fragmented_packet) => fragmented_packet.message_acks(),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use lightyear_derive::ChannelInternal;

    use crate::packet::message::{FragmentData, SingleData};
    use crate::packet::packet::{FragmentedPacket, PacketData, SinglePacket};
    use crate::packet::packet_manager::PacketManager;
    use crate::packet::packet_type::PacketType;
    use crate::packet::wrapping_id::MessageId;
    use crate::{
        BitSerializable, ChannelDirection, ChannelKind, ChannelMode, ChannelRegistry,
        ChannelSettings, MessageContainer, ReadBuffer, ReadWordBuffer, WriteBuffer,
        WriteWordBuffer,
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
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let mut packet = SinglePacket::new();

        packet.add_message(0, SingleData::new(None, Bytes::from("hello")));
        packet.add_message(0, SingleData::new(None, Bytes::from("world")));
        packet.add_message(1, SingleData::new(None, Bytes::from("!")));

        assert_eq!(packet.num_messages(), 3);
    }

    #[test]
    fn test_encode_single_packet() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let mut packet = SinglePacket::new();

        let mut write_buffer = WriteWordBuffer::with_capacity(50);
        let message1 = SingleData::new(None, Bytes::from("hello"));
        let message2 = SingleData::new(None, Bytes::from("world"));
        let message3 = SingleData::new(None, Bytes::from("!"));

        packet.add_message(0, message1.clone().into());
        packet.add_message(0, message2.clone().into());
        packet.add_message(1, message3.clone().into());
        // add a channel with no messages
        packet.add_channel(2);

        packet.encode(&mut write_buffer);
        let packet_bytes = write_buffer.finish_write();

        // Encode manually
        let mut expected_write_buffer = WriteWordBuffer::with_capacity(50);
        // channel id
        expected_write_buffer.serialize(&0u16)?;
        // messages, with continuation bit
        expected_write_buffer.serialize(&true)?;
        message1.encode(&mut expected_write_buffer)?;
        expected_write_buffer.serialize(&true)?;
        message2.encode(&mut expected_write_buffer)?;
        expected_write_buffer.serialize(&false)?;
        // channel continue bit
        expected_write_buffer.serialize(&true)?;
        // channel id
        expected_write_buffer.serialize(&1u16)?;
        // messages with continuation bit
        expected_write_buffer.serialize(&true)?;
        message3.encode(&mut expected_write_buffer)?;
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
        let decoded_packet = SinglePacket::decode(&mut reader)?;

        assert_eq!(decoded_packet.num_messages(), 3);
        assert_eq!(packet, decoded_packet);
        Ok(())
    }

    #[test]
    fn test_encode_fragmented_packet() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let bytes = Bytes::from(vec![0; 10]);
        let fragment = FragmentData {
            message_id: MessageId(0),
            fragment_id: 2,
            num_fragments: 3,
            bytes: bytes.clone(),
        };
        let mut packet = FragmentedPacket::new(*channel_id, fragment.clone());

        let mut write_buffer = WriteWordBuffer::with_capacity(100);
        let message1 = SingleData::new(None, Bytes::from("hello"));
        let message2 = SingleData::new(None, Bytes::from("world"));
        let message3 = SingleData::new(None, Bytes::from("!"));

        packet.packet.add_message(0, message1.clone().into());
        packet.packet.add_message(0, message2.clone().into());
        packet.packet.add_message(1, message3.clone().into());
        // add a channel with no messages
        packet.packet.add_channel(2);

        packet.encode(&mut write_buffer);
        let packet_bytes = write_buffer.finish_write();

        let mut reader = ReadWordBuffer::start_read(packet_bytes);
        let decoded_packet = FragmentedPacket::decode(&mut reader)?;

        assert_eq!(decoded_packet.packet.num_messages(), 3);
        assert_eq!(packet, decoded_packet);
        Ok(())
    }
}
