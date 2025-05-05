/// Defines the [`Packet`] struct
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::message::MessageAck;
use crate::packet::packet_builder::Payload;
use crate::protocol::channel::ChannelId;
use crate::serialize::ToBytes;
use crate::utils::wrapping_id::wrapping_id;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

// Internal id that we assign to each packet sent over the network
wrapping_id!(PacketId);

/// Number of bytes to write the header
const HEADER_BYTES: usize = 11;

/// The maximum number of bytes for a message before it is fragmented
/// MAX_PACKET_SIZE - HEADER_BYTES - 1 (channel_net_id) - 6 (message_id/fragment_id/num_fragments) - 2 (num bytes in fragment)
// NOTE: this considers that we use 2 bytes for the fragment id and num_fragments, but in reality we are using
//  varints so it could be more or less!
pub(crate) const FRAGMENT_SIZE: usize = MAX_PACKET_SIZE - HEADER_BYTES - 9;

/// Data structure that will help us write the packet
#[derive(Debug)]
pub(crate) struct Packet {
    pub(crate) payload: Payload,
    /// Content of the packet so we can map from channel id to message ids
    pub(crate) message_acks: Vec<(ChannelId, MessageAck)>,
    pub(crate) packet_id: PacketId,
    // How many bytes we know we are going to have to write in the packet, but haven't written yet
    pub(crate) prewritten_size: usize,
}

impl Packet {
    /// Check that we can still fit some data in the buffer
    pub(crate) fn can_fit(&self, size: usize) -> bool {
        self.payload.len() + size + self.prewritten_size <= MAX_PACKET_SIZE
    }

    /// Check if we can write a channel_id + the number of messages in the packet.
    /// If we can, reserve some space for it
    pub(crate) fn can_fit_channel(&mut self, channel_id: ChannelId) -> bool {
        let size = channel_id.bytes_len() + 1;
        // size of the channel + 1 for the number of messages
        let can_fit = self.can_fit(channel_id.bytes_len() + 1);
        if can_fit {
            // reserve the space to write the channel
            self.prewritten_size += size;
        }
        can_fit
    }

    pub(crate) fn num_messages(&self) -> usize {
        self.message_acks.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.message_acks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use crate::prelude::PacketError;
    use crate::packet::header::PacketHeader;
    use crate::packet::packet_type::PacketType;
    use crate::packet::message::{SingleData, FragmentData};
    use bevy::platform::collections::HashMap;
    use bevy::prelude::{default, Reflect};

    use lightyear_macros::ChannelInternal;
    use crate::prelude::{ChannelMode, ChannelRegistry, ChannelSettings};
    use crate::protocol::channel::ChannelId;
    use crate::serialize::reader::{ReadVarInt};
    use crate::serialize::ToBytes;
    use super::*;

    impl Packet {
        /// For tests, parse the packet so that we can inspect the contents
        /// For production, parse the packets directly into messages to not allocate
        /// an intermediary data structure
        pub(crate) fn parse_packet_payload(
            self,
        ) -> Result<HashMap<ChannelId, Vec<Bytes>>, PacketError> {
            let mut cursor = self.payload.into();
            let mut res: HashMap<ChannelId, Vec<Bytes>> = HashMap::default();
            let header = PacketHeader::from_bytes(&mut cursor)?;

            if header.get_packet_type() == PacketType::DataFragment {
                // read the fragment data
                let channel_id = ChannelId::from_bytes(&mut cursor)?;
                let fragment_data = FragmentData::from_bytes(&mut cursor)?;
                res.entry(channel_id).or_default().push(fragment_data.bytes);
            }
            // read single message data
            // TODO: avoid infinite loop here!
            while cursor.has_remaining() {
                let channel_id = ChannelId::from_bytes(&mut cursor)?;
                let num_messages = cursor.read_varint()?;
                for i in 0..num_messages {
                    let single_data = SingleData::from_bytes(&mut cursor)?;
                    res.entry(channel_id).or_default().push(single_data.bytes);
                }
            }
            Ok(res)
        }
    }

    #[derive(ChannelInternal, Reflect)]
    struct Channel1;

    #[derive(ChannelInternal, Reflect)]
    struct Channel2;

    fn get_channel_registry() -> ChannelRegistry {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        let mut c = ChannelRegistry::default();
        c.add_channel::<Channel1>(settings.clone());
        c.add_channel::<Channel2>(settings.clone());
        c
    }

    // #[test]
    // fn test_single_packet_add_messages() {
    //     let channel_registry = get_channel_registry();
    //     let manager = PacketBuilder::new(1.5);
    //     let mut packet = SinglePacket::new();
    //
    //     packet.add_message(0, SingleData::new(None, Bytes::from("hello")));
    //     packet.add_message(0, SingleData::new(None, Bytes::from("world")));
    //     packet.add_message(1, SingleData::new(None, Bytes::from("!")));
    //
    //     assert_eq!(packet.num_messages(), 3);
    // }
    //
    // #[test]
    // fn test_encode_single_packet() -> anyhow::Result<()> {
    //     let channel_registry = get_channel_registry();
    //     let manager = PacketBuilder::new(1.5);
    //     let mut packet = SinglePacket::new();
    //
    //     let mut write_buffer = BitcodeWriter::with_capacity(50);
    //     let message1 = SingleData::new(None, Bytes::from("hello"));
    //     let message2 = SingleData::new(None, Bytes::from("world"));
    //     let message3 = SingleData::new(None, Bytes::from("!"));
    //
    //     packet.add_message(0, message1.clone());
    //     packet.add_message(0, message2.clone());
    //     packet.add_message(1, message3.clone());
    //     // add a channel with no messages
    //     packet.add_channel(2);
    //
    //     packet.encode(&mut write_buffer)?;
    //     let packet_bytes = write_buffer.finish_write();
    //
    //     // Encode manually
    //     let mut expected_write_buffer = BitcodeWriter::with_capacity(50);
    //     // channel id
    //     expected_write_buffer.encode(&0u16, Gamma)?;
    //     // messages, with continuation bit
    //     expected_write_buffer.serialize(&true)?;
    //     message1.encode(&mut expected_write_buffer)?;
    //     expected_write_buffer.serialize(&true)?;
    //     message2.encode(&mut expected_write_buffer)?;
    //     expected_write_buffer.serialize(&false)?;
    //     // channel continue bit
    //     expected_write_buffer.serialize(&true)?;
    //     // channel id
    //     expected_write_buffer.encode(&1u16, Gamma)?;
    //     // messages with continuation bit
    //     expected_write_buffer.serialize(&true)?;
    //     message3.encode(&mut expected_write_buffer)?;
    //     expected_write_buffer.serialize(&false)?;
    //     // channel continue bit
    //     expected_write_buffer.serialize(&true)?;
    //     // channel id
    //     expected_write_buffer.encode(&2u16, Gamma)?;
    //     // messages with continuation bit
    //     expected_write_buffer.serialize(&false)?;
    //     // channel continue bit
    //     expected_write_buffer.serialize(&false)?;
    //
    //     let expected_packet_bytes = expected_write_buffer.finish_write();
    //
    //     assert_eq!(packet_bytes, expected_packet_bytes);
    //
    //     let mut reader = BitcodeReader::start_read(packet_bytes);
    //     let decoded_packet = SinglePacket::decode(&mut reader)?;
    //
    //     assert_eq!(decoded_packet.num_messages(), 3);
    //     assert_eq!(packet, decoded_packet);
    //     Ok(())
    // }
    //
    // #[test]
    // fn test_encode_fragmented_packet() -> anyhow::Result<()> {
    //     let channel_registry = get_channel_registry();
    //     let manager = PacketBuilder::new(1.5);
    //     let channel_kind = ChannelKind::of::<Channel1>();
    //     let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
    //     let bytes = Bytes::from(vec![0; 10]);
    //     let fragment = FragmentData {
    //         message_id: MessageId(0),
    //         fragment_id: 2,
    //         num_fragments: 3,
    //         bytes: bytes.clone(),
    //     };
    //     let mut packet = FragmentedPacket::new(*channel_id, fragment.clone());
    //
    //     let mut write_buffer = BitcodeWriter::with_capacity(100);
    //     let message1 = SingleData::new(None, Bytes::from("hello"));
    //     let message2 = SingleData::new(None, Bytes::from("world"));
    //     let message3 = SingleData::new(None, Bytes::from("!"));
    //
    //     packet.packet.add_message(0, message1.clone());
    //     packet.packet.add_message(0, message2.clone());
    //     packet.packet.add_message(1, message3.clone());
    //     // add a channel with no messages
    //     packet.packet.add_channel(2);
    //
    //     packet.encode(&mut write_buffer)?;
    //     let packet_bytes = write_buffer.finish_write();
    //
    //     let mut reader = BitcodeReader::start_read(packet_bytes);
    //     let decoded_packet = FragmentedPacket::decode(&mut reader)?;
    //
    //     assert_eq!(decoded_packet.packet.num_messages(), 3);
    //     assert_eq!(packet, decoded_packet);
    //     Ok(())
    // }
    //
    // #[test]
    // fn test_encode_fragmented_packet_no_single_data() -> anyhow::Result<()> {
    //     let channel_registry = get_channel_registry();
    //     let manager = PacketBuilder::new(1.5);
    //     let channel_kind = ChannelKind::of::<Channel1>();
    //     let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
    //     let bytes = Bytes::from(vec![0; 10]);
    //     let fragment = FragmentData {
    //         message_id: MessageId(0),
    //         fragment_id: 2,
    //         num_fragments: 3,
    //         bytes: bytes.clone(),
    //     };
    //     let packet = FragmentedPacket::new(*channel_id, fragment.clone());
    //
    //     let mut write_buffer = BitcodeWriter::with_capacity(100);
    //
    //     packet.encode(&mut write_buffer)?;
    //     let packet_bytes = write_buffer.finish_write();
    //
    //     let mut reader = BitcodeReader::start_read(packet_bytes);
    //     let decoded_packet = FragmentedPacket::decode(&mut reader)?;
    //
    //     assert_eq!(decoded_packet.packet.num_messages(), 0);
    //     assert_eq!(packet, decoded_packet);
    //     Ok(())
    // }
}
