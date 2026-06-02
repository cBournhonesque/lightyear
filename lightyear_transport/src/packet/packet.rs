/// Defines the [`Packet`] struct
use crate::channel::registry::ChannelId;
use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentIndex, MessageId};
use crate::packet::packet_builder::Payload;
use alloc::vec::Vec;
use lightyear_serde::{ToBytes, varint::varint_len};
use lightyear_utils::wrapping_id;

// Internal id that we assign to each packet sent over the network
wrapping_id!(PacketId);

pub(crate) const MAX_PACKET_SIZE: usize = 1200;
/// Number of bytes written by [`PacketHeader::to_bytes`].
///
/// Keep this in sync with `PacketHeader` because [`FRAGMENT_SIZE`] depends on it.
/// If this value is too small, fragment packets can overflow the 1200-byte MTU even when
/// the fragment payload itself appears to fit.
pub(crate) const HEADER_BYTES: usize = PacketHeader::BYTES;

const MAX_FRAGMENT_CHANNEL_ID_BYTES: usize = varint_len(u16::MAX as u64);
const MAX_FRAGMENT_ID_BYTES: usize = 8;
const MAX_FRAGMENT_COUNT_BYTES: usize = 8;
const FRAGMENT_COMPRESSION_BYTES: usize = 1;
const MAX_FRAGMENT_LENGTH_BYTES: usize = varint_len(MAX_PACKET_SIZE as u64);
const MAX_FRAGMENT_METADATA_BYTES: usize = MAX_FRAGMENT_CHANNEL_ID_BYTES
    + 4 // MessageId
    + MAX_FRAGMENT_ID_BYTES
    + MAX_FRAGMENT_COUNT_BYTES
    + FRAGMENT_COMPRESSION_BYTES
    + MAX_FRAGMENT_LENGTH_BYTES;

/// The maximum number of payload bytes in a transport fragment.
///
/// This reserves enough room for the packet header plus the largest supported encoded fragment
/// metadata. The receiver assumes every non-final fragment has this fixed size when
/// reconstructing the original message.
pub(crate) const FRAGMENT_SIZE: usize =
    MAX_PACKET_SIZE - HEADER_BYTES - MAX_FRAGMENT_METADATA_BYTES;

/// Metadata about messages included in a packet.
///
/// With the `metrics` feature enabled, this contains every message written into the packet so
/// channel metrics can be emitted after the final packet passes the bandwidth limiter.
///
/// Without `metrics`, this only contains messages with an id, because the send path only needs
/// packet-local metadata for ack/retry bookkeeping.
#[derive(Debug, PartialEq)]
pub(crate) struct MessageMetadata {
    pub(crate) channel: ChannelId,
    pub(crate) message: Option<MessageId>,
    // if the message is fragmented, we store the total number of fragments
    pub(crate) fragment_index: Option<FragmentIndex>,
    pub(crate) num_fragments: Option<u64>,
    // Size of the message in bytes (for fragments, it's only the size of the fragment)
    #[cfg(feature = "metrics")]
    pub(crate) num_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PacketCompressionInfo {
    pub(crate) original_len: usize,
    pub(crate) compressed_len: usize,
}

/// Data structure that will help us write the packet
#[derive(Debug)]
pub(crate) struct Packet {
    pub(crate) payload: Payload,
    /// Packet-local message metadata.
    ///
    /// See [`MessageMetadata`] for the feature-dependent rule that decides whether id-less
    /// messages are recorded.
    pub(crate) messages: Vec<MessageMetadata>,
    pub(crate) packet_id: PacketId,
    // How many bytes we know we are going to have to write in the packet, but haven't written yet
    pub(crate) prewritten_size: usize,
    pub(crate) compression: Option<PacketCompressionInfo>,
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
        self.messages.len()
    }

    pub(crate) fn record_message_metadata(
        &mut self,
        channel: ChannelId,
        message: Option<MessageId>,
        fragment_index: Option<FragmentIndex>,
        num_fragments: Option<u64>,
        #[cfg(feature = "metrics")] num_bytes: usize,
    ) {
        // In metrics builds, every message needs metadata so channel/send_* can be emitted only
        // after the final packet passes bandwidth quota. In non-metrics builds, id-less entries
        // are skipped to keep packet metadata limited to ack/retry bookkeeping.
        if cfg!(feature = "metrics") || message.is_some() {
            self.messages.push(MessageMetadata {
                channel,
                message,
                fragment_index,
                num_fragments,
                #[cfg(feature = "metrics")]
                num_bytes,
            });
        }
    }

    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use crate::packet::header::{PacketHeader, PacketHeaderManager};
    use crate::packet::message::{FragmentData, SingleData};
    use crate::packet::packet_type::PacketType;
    use bevy_app::App;
    use bevy_platform::collections::HashMap;
    use bevy_reflect::Reflect;
    use bevy_utils::default;
    use bytes::Bytes;
    use lightyear_core::prelude::Tick;

    use super::*;
    use crate::channel::builder::{ChannelMode, ChannelSettings};
    use crate::channel::registry::{AppChannelExt, ChannelRegistry};
    use crate::packet::error::PacketError;
    use lightyear_serde::reader::ReadInteger;
    use lightyear_serde::{SerializationError, ToBytes};

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
                let num_messages = cursor.read_u8().map_err(SerializationError::from)?;
                for _ in 0..num_messages {
                    let single_data = SingleData::from_bytes(&mut cursor)?;
                    res.entry(channel_id).or_default().push(single_data.bytes);
                }
            }
            Ok(res)
        }
    }

    #[derive(Reflect)]
    struct Channel1;

    #[derive(Reflect)]
    struct Channel2;

    fn get_channel_registry() -> ChannelRegistry {
        let mut app = App::new();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        app.init_resource::<ChannelRegistry>();
        app.add_channel::<Channel1>(settings);
        app.add_channel::<Channel2>(settings);
        app.world_mut()
            .remove_resource::<ChannelRegistry>()
            .unwrap()
    }

    #[test]
    fn header_bytes_constant_matches_packet_header_encoding() {
        let header = PacketHeaderManager::new(1.5).prepare_send_packet_header(
            PacketType::Data,
            core::time::Duration::default(),
            Tick(3),
        );

        let mut writer = Vec::new();
        header.to_bytes(&mut writer).unwrap();

        assert_eq!(writer.len(), HEADER_BYTES);
        assert_eq!(header.bytes_len(), HEADER_BYTES);
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
