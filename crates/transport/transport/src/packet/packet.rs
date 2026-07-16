/// Defines the [`Packet`] struct
use crate::channel::registry::ChannelId;
use crate::channel::registry::ChannelKind;
use crate::packet::header::PacketHeader;
use crate::packet::message::{FragmentIndex, MessageId, SendMessageKey};
use crate::packet::packet_builder::Payload;
use alloc::vec::Vec;
use lightyear_link::DEFAULT_MTU;
use lightyear_serde::varint::varint_len;
use lightyear_utils::wrapping_id;

// Internal id that we assign to each packet sent over the network
wrapping_id!(PacketId);

/// Number of bytes written by [`PacketHeader::to_bytes`].
///
/// Keep this in sync with `PacketHeader` because [`fragment_size_for_mtu`] depends on it.
pub(crate) const HEADER_BYTES: usize = PacketHeader::BYTES;

const MAX_FRAGMENT_CHANNEL_ID_BYTES: usize = varint_len(u16::MAX as u64);
const MAX_FRAGMENT_ID_BYTES: usize = 8;
const MAX_FRAGMENT_COUNT_BYTES: usize = 8;
const INITIAL_FRAGMENT_COMPRESSION_BYTES: usize = 1;
const FIXED_FRAGMENT_METADATA_BYTES: usize = MAX_FRAGMENT_CHANNEL_ID_BYTES
    + 4 // MessageId
    + MAX_FRAGMENT_ID_BYTES
    + MAX_FRAGMENT_COUNT_BYTES
    + INITIAL_FRAGMENT_COMPRESSION_BYTES;

/// Smallest link MTU which can carry one byte with the largest supported fragment identifiers.
pub(crate) const MIN_PACKET_SIZE: usize = FIXED_FRAGMENT_METADATA_BYTES
    + HEADER_BYTES
    + 1 // encoded fragment byte length
    + 1; // payload

/// Returns the fixed fragment payload size to use for a link's stable minimum MTU.
///
/// Both peers derive this from the link's stable minimum MTU. Reserving the largest identifier
/// encodings ensures every fragment produced at this size fits even for long-running sessions and
/// very large logical messages.
pub(crate) const fn fragment_size_for_mtu(mtu: usize) -> Option<usize> {
    let mtu_varint_bytes = varint_len(mtu as u64);
    // Fragment payloads use a varint length prefix.
    let overhead = HEADER_BYTES + FIXED_FRAGMENT_METADATA_BYTES + mtu_varint_bytes;
    match mtu.checked_sub(overhead) {
        Some(fragment_size) if fragment_size > 0 => Some(fragment_size),
        _ => None,
    }
}

/// The default maximum number of payload bytes in a transport fragment.
///
/// Links with a non-default minimum MTU use [`fragment_size_for_mtu`] instead.
pub(crate) const FRAGMENT_SIZE: usize = match fragment_size_for_mtu(DEFAULT_MTU) {
    Some(fragment_size) => fragment_size,
    None => panic!("default link MTU cannot hold transport fragment metadata"),
};

/// Metadata about messages included in a packet.
///
/// There is one entry per staged message. The commit handle updates channel-owned pending data only
/// after the packet enters `Link.send`; with `metrics`, the same entry also carries the byte count
/// used for post-admission metrics.
#[derive(Debug, PartialEq)]
pub(crate) struct MessageMetadata {
    pub(crate) channel: ChannelId,
    pub(crate) message: Option<MessageId>,
    // if the message is fragmented, we store the total number of fragments
    pub(crate) fragment_index: Option<FragmentIndex>,
    pub(crate) num_fragments: Option<u64>,
    /// Channel-owned pending state to update after this packet is admitted to `Link.send`.
    pub(crate) commit: SendCommit,
    // Size of the message in bytes (for fragments, it's only the size of the fragment)
    #[cfg(feature = "metrics")]
    pub(crate) num_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SendCommit {
    pub(crate) channel_kind: ChannelKind,
    pub(crate) key: SendMessageKey,
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
    /// One packet-local metadata entry per staged message.
    pub(crate) messages: Vec<MessageMetadata>,
    pub(crate) packet_id: PacketId,
    pub(crate) compression: Option<PacketCompressionInfo>,
}

impl Packet {
    pub(crate) fn num_messages(&self) -> usize {
        self.messages.len()
    }

    pub(crate) fn record_message_metadata(
        &mut self,
        channel: ChannelId,
        message: Option<MessageId>,
        fragment_index: Option<FragmentIndex>,
        num_fragments: Option<u64>,
        commit: SendCommit,
        #[cfg(feature = "metrics")] num_bytes: usize,
    ) {
        self.messages.push(MessageMetadata {
            channel,
            message,
            fragment_index,
            num_fragments,
            commit,
            #[cfg(feature = "metrics")]
            num_bytes,
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::packet::header::{PacketHeader, PacketHeaderManager};
    use crate::packet::message::{FragmentData, SingleData};
    use crate::packet::packet_type::PacketType;
    use bevy_platform::collections::HashMap;
    use bytes::Bytes;
    use lightyear_core::prelude::Tick;

    use super::*;
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

    #[test]
    fn header_bytes_constant_matches_packet_header_encoding() {
        let header =
            PacketHeaderManager::new(1.5).preview_send_packet_header(PacketType::Data, Tick(3));

        let mut writer = Vec::new();
        header.to_bytes(&mut writer).unwrap();

        assert_eq!(writer.len(), HEADER_BYTES);
        assert_eq!(header.bytes_len(), HEADER_BYTES);
    }
}
