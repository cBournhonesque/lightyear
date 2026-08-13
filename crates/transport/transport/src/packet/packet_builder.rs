//! Module to take a buffer of messages to send and build packets
use crate::channel::registry::ChannelId;
use crate::packet::compression::{
    CompressionConfig, CompressionOutcome, CompressionScratch, compress_packet,
    evaluate_packet_compression,
};
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::{MessageData, SendCandidate};
use crate::packet::nack::PacketNackSettings;
use crate::packet::packet::{HEADER_BYTES, MessageMetadata, Packet, SendCommit};
use crate::packet::packet_type::PacketType;
use alloc::vec::Vec;
use bytes::{Bytes, BytesMut};
use lightyear_core::{buffer_pool::BufferPool, tick::Tick};
use lightyear_link::{DEFAULT_MTU, SendPayload};
use lightyear_serde::{SerializationError, ToBytes};
use no_std_io2::io::{self, Write};
use tracing::trace;

/// Borrowed adapter for the serialization traits, which are based on `no_std_io2::Write`.
/// Packet storage itself remains a plain [`BytesMut`].
struct PayloadWriter<'a>(&'a mut BytesMut);

impl Write for PayloadWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn write_to_payload<T: ToBytes + ?Sized>(
    value: &T,
    payload: &mut BytesMut,
) -> Result<(), SerializationError> {
    value.to_bytes(&mut PayloadWriter(payload))
}

const MAX_MESSAGES_PER_CHANNEL_BATCH: u8 = u8::MAX;
const MAX_RETAINED_MESSAGE_METADATA_CAPACITY: usize = 100;
/// Bounds both the pending scan and retained payload memory for one Transport.
const MAX_RETAINED_PACKET_PAYLOADS: usize = 64;

/// We use `Bytes` on the receive side because we want to be able to refer to sub-slices of the original
/// packet without allocating.
///
/// e.g. we receive 1200 bytes from the network, we want to read parts of it (header, channel) but then
/// store subslices in receiver channels without allocating.
pub type RecvPayload = Bytes;

/// Position in the ordered send-candidate scratch for the current flush.
#[derive(Debug, Default)]
pub(crate) struct CandidateCursor {
    index: usize,
    single_index: usize,
}

/// Builder-local state for the currently open batch of single messages.
///
/// A batch is encoded as a channel ID, a one-byte message count, and that many consecutive
/// [`SingleData`](crate::packet::message::SingleData) values. Grouping messages this way avoids
/// repeating the channel ID for every message. This state is not serialized as a struct: `count`
/// mirrors the serialized count byte at `count_offset` so the builder can update it in place.
/// Once `count` reaches [`u8::MAX`], another batch is started, even for the same channel.
#[derive(Clone, Copy, Debug)]
struct SingleBatchState {
    /// Channel shared by every message in this batch.
    channel_id: ChannelId,
    /// Offset of the serialized message-count byte in the packet payload.
    count_offset: usize,
    /// Number of messages in this batch and the value stored at `count_offset`.
    count: u8,
}

/// `PacketBuilder` handles the process of creating a packet (writing the header and packing the
/// messages into packets)
#[derive(Debug)]
pub(crate) struct PacketBuilder {
    pub(crate) header_manager: PacketHeaderManager,
    buffer_pool: PacketBufferPool,
    compression_output: Vec<u8>,
}

/// Reusable packet-builder storage.
///
/// Payload allocation recycling is delegated to the shared [`BufferPool`]. This wrapper also
/// retains packet-specific message metadata lists.
#[derive(Debug)]
pub(crate) struct PacketBufferPool {
    payloads: BufferPool,
    message_metadata: Vec<Vec<MessageMetadata>>,
}

impl PacketBufferPool {
    pub(crate) fn new(payload_capacity: usize) -> Self {
        Self {
            payloads: BufferPool::new(payload_capacity, MAX_RETAINED_PACKET_PAYLOADS),
            message_metadata: Vec::new(),
        }
    }

    pub(crate) fn take_payload(&mut self) -> BytesMut {
        self.payloads.take()
    }

    fn set_payload_capacity(&mut self, payload_capacity: usize) {
        self.payloads.set_buffer_capacity(payload_capacity);
    }

    /// Promotes split tails whose immutable views have all been dropped to the ready list.
    ///
    /// Dropping the last [`Bytes`] view does not mutate its sibling [`BytesMut`] or automatically
    /// restore the sibling's capacity. It only makes `try_reclaim` capable of recovering the full
    /// allocation. This sweep performs that explicit recovery once per transport send pass.
    fn reclaim_pending_payloads(&mut self) {
        self.payloads.reclaim_pending();
    }

    pub(crate) fn take_message_metadata(&mut self) -> Vec<MessageMetadata> {
        self.message_metadata
            .pop()
            .map(|mut messages| {
                messages.clear();
                messages
            })
            .unwrap_or_default()
    }

    /// Recycles a staged packet that was not handed to `Link`.
    pub(crate) fn recycle_packet(&mut self, packet: Packet) {
        let Packet {
            payload, messages, ..
        } = packet;
        self.payloads.recycle(payload);
        self.recycle_message_metadata(messages);
    }

    pub(crate) fn recycle_message_metadata(&mut self, mut messages: Vec<MessageMetadata>) {
        let capacity = messages.capacity();
        if (1..=MAX_RETAINED_MESSAGE_METADATA_CAPACITY).contains(&capacity) {
            messages.clear();
            self.message_metadata.push(messages);
        }
    }

    fn payload_pool_misses(&self) -> usize {
        self.payloads.misses()
    }
}

impl Default for PacketBuilder {
    fn default() -> Self {
        Self::with_nack_settings(PacketNackSettings::default())
    }
}

impl PacketBuilder {
    pub fn new(nack_rtt_multiplier: f32) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(nack_rtt_multiplier),
            buffer_pool: PacketBufferPool::new(DEFAULT_MTU),
            compression_output: Vec::new(),
        }
    }

    pub(crate) fn with_nack_settings(nack_settings: PacketNackSettings) -> Self {
        Self {
            header_manager: PacketHeaderManager::with_nack_settings(nack_settings),
            buffer_pool: PacketBufferPool::new(DEFAULT_MTU),
            compression_output: Vec::new(),
        }
    }

    pub(crate) fn nack_settings(&self) -> PacketNackSettings {
        self.header_manager.nack_settings
    }

    pub(crate) fn set_nack_settings(&mut self, settings: PacketNackSettings) {
        self.header_manager.nack_settings = settings;
    }

    pub(crate) fn recycle_packet(&mut self, packet: Packet) {
        self.buffer_pool.recycle_packet(packet);
    }

    pub(crate) fn recycle_message_metadata_list(&mut self, messages: Vec<MessageMetadata>) {
        self.buffer_pool.recycle_message_metadata(messages);
    }

    /// Prepares the packet buffer pool for one transport send pass.
    ///
    /// Reclaiming pending tails here ensures each shared tail is probed at most once per pass. All
    /// packets built during the pass can then take known-ready buffers without scanning the pool.
    pub(crate) fn begin_send(&mut self, mtu: usize) {
        self.buffer_pool.set_payload_capacity(mtu);
        self.buffer_pool.reclaim_pending_payloads();
    }

    #[cfg(feature = "test_utils")]
    pub(crate) fn payload_pool_misses(&self) -> usize {
        self.buffer_pool.payload_pool_misses()
    }

    /// Transfers a successfully staged packet payload across the `Link` ownership boundary.
    ///
    /// The handoff has four steps:
    ///
    /// 1. Check the complete buffer's capacity before splitting. After the split, the tail exposes
    ///    only its remaining portion and cannot reveal whether the original allocation was larger
    ///    than the MTU.
    /// 2. [`BufferPool::split_for_handoff`] moves the initialized prefix into another
    ///    [`BytesMut`] and retains the empty sibling tail internally.
    /// 3. Freezing the prefix produces the ordinary [`SendPayload`] (`Bytes`) expected by Link and
    ///    every IO backend, without copying the packet bytes.
    /// 4. The retained tail is normally pending because the returned [`Bytes`] still shares its
    ///    allocation. Dropping the final `Bytes` clone merely makes the tail reclaimable; the next
    ///    [`Self::begin_send`] sweep explicitly restores its capacity.
    pub(crate) fn take_send_payload(&mut self, packet: &mut Packet) -> SendPayload {
        self.buffer_pool
            .payloads
            .split_for_handoff(core::mem::take(&mut packet.payload))
            .freeze()
    }

    /// Build a header-only acknowledgement packet.
    pub(crate) fn build_ack_only_packet(
        &mut self,
        current_tick: Tick,
        mtu: usize,
    ) -> Result<Packet, PacketError> {
        self.buffer_pool.set_payload_capacity(mtu);
        if mtu < HEADER_BYTES {
            return Err(PacketError::PacketTooLarge {
                actual: HEADER_BYTES,
                mtu,
            });
        }
        Ok(self.new_staged_packet(PacketType::AckOnly, current_tick)?)
    }

    /// Stage the next packet from globally ordered, channel-owned candidates.
    ///
    /// This method only consumes candidate snapshots. It deliberately does not mutate channel
    /// queues, reliable retry timestamps, packet ids, sent-packet tracking, or bandwidth quota.
    /// The caller commits those transitions after the returned packet enters `Link.send`.
    pub(crate) fn build_next_packet(
        &mut self,
        current_tick: Tick,
        candidates: &[SendCandidate],
        cursor: &mut CandidateCursor,
        compression: CompressionConfig,
        mtu: usize,
        compression_scratch: &mut CompressionScratch,
    ) -> Result<Option<Packet>, PacketError> {
        self.buffer_pool.set_payload_capacity(mtu);
        // Final fragments can consume singles which appear later in their priority group. Skip
        // those candidates once the main cursor reaches the single portion of the group.
        while cursor.index < cursor.single_index
            && candidates
                .get(cursor.index)
                .is_some_and(|candidate| matches!(candidate.message.data, MessageData::Single(_)))
        {
            cursor.index += 1;
        }
        let Some(first) = candidates.get(cursor.index) else {
            return Ok(None);
        };

        match &first.message.data {
            MessageData::Fragment(fragment) => {
                let effective_priority = first.effective_priority;
                let mut packet = self.new_staged_packet(PacketType::DataFragment, current_tick)?;
                write_to_payload(&first.channel_id, &mut packet.payload)?;
                write_to_payload(fragment, &mut packet.payload)?;
                packet.record_message_metadata(
                    first.channel_id,
                    Some(fragment.message_id),
                    Some(fragment.fragment_id),
                    SendCommit {
                        channel_kind: first.channel_kind,
                        key: first.key,
                    },
                    #[cfg(feature = "metrics")]
                    fragment.bytes.len(),
                );
                cursor.index += 1;

                // The wire format permits singles after the final fragment. Preserve the current
                // behavior of fitting that tail against the uncompressed MTU. The separate
                // single cursor can look past other fragments in the same priority group, letting
                // every fragmented message use its final packet's remaining space.
                if fragment.is_last_fragment() {
                    let mut batch = None;
                    cursor.single_index = cursor.single_index.max(cursor.index);
                    while let Some(candidate) = candidates.get(cursor.single_index) {
                        if candidate.effective_priority.total_cmp(&effective_priority)
                            != core::cmp::Ordering::Equal
                        {
                            break;
                        }
                        if matches!(candidate.message.data, MessageData::Fragment(_)) {
                            cursor.single_index += 1;
                            continue;
                        }
                        if !Self::try_append_single_candidate(
                            &mut packet,
                            candidate,
                            &mut batch,
                            CompressionConfig::DISABLED,
                            mtu,
                            compression_scratch,
                            &mut self.compression_output,
                        )? {
                            break;
                        }
                        cursor.single_index += 1;
                    }
                }

                if packet.payload.len() > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: packet.payload.len(),
                        mtu,
                    });
                }
                Ok(Some(packet))
            }
            MessageData::Single(_) => {
                let mut packet = self.new_staged_packet(PacketType::Data, current_tick)?;
                let mut batch = None;
                while let Some(candidate) = candidates.get(cursor.index) {
                    if !matches!(candidate.message.data, MessageData::Single(_)) {
                        break;
                    }
                    if !Self::try_append_single_candidate(
                        &mut packet,
                        candidate,
                        &mut batch,
                        compression,
                        mtu,
                        compression_scratch,
                        &mut self.compression_output,
                    )? {
                        break;
                    }
                    cursor.index += 1;
                }
                cursor.single_index = cursor.single_index.max(cursor.index);

                if packet.messages.is_empty() {
                    let candidate = &candidates[cursor.index];
                    return Err(PacketError::PacketTooLarge {
                        actual: HEADER_BYTES
                            + candidate.channel_id.bytes_len()
                            + 1
                            + candidate.message.data.bytes_len(),
                        mtu,
                    });
                }
                Ok(Some(self.finish_compression_aware_packet(
                    packet,
                    compression,
                    mtu,
                    compression_scratch,
                )?))
            }
        }
    }

    fn new_staged_packet(
        &mut self,
        packet_type: PacketType,
        current_tick: Tick,
    ) -> Result<Packet, SerializationError> {
        let mut payload = self.buffer_pool.take_payload();
        let messages = self.buffer_pool.take_message_metadata();
        let header = self
            .header_manager
            .preview_send_packet_header(packet_type, current_tick);
        write_to_payload(&header, &mut payload)?;
        Ok(Packet {
            payload,
            messages,
            packet_id: header.packet_id,
            compression: None,
        })
    }

    fn try_append_single_candidate(
        packet: &mut Packet,
        candidate: &SendCandidate,
        batch: &mut Option<SingleBatchState>,
        compression: CompressionConfig,
        mtu: usize,
        compression_scratch: &mut CompressionScratch,
        compression_output: &mut Vec<u8>,
    ) -> Result<bool, PacketError> {
        let MessageData::Single(message) = &candidate.message.data else {
            return Ok(false);
        };

        let payload_len = packet.payload.len();
        let metadata_len = packet.messages.len();
        let previous_batch = *batch;

        match batch {
            Some(state)
                if state.channel_id == candidate.channel_id
                    && state.count < MAX_MESSAGES_PER_CHANNEL_BATCH =>
            {
                state.count += 1;
                packet.payload.as_mut()[state.count_offset] = state.count;
            }
            _ => {
                write_to_payload(&candidate.channel_id, &mut packet.payload)?;
                let count_offset = packet.payload.len();
                write_to_payload(&1u8, &mut packet.payload)?;
                *batch = Some(SingleBatchState {
                    channel_id: candidate.channel_id,
                    count_offset,
                    count: 1,
                });
            }
        }

        write_to_payload(message, &mut packet.payload)?;
        packet.record_message_metadata(
            candidate.channel_id,
            message.id,
            None,
            SendCommit {
                channel_kind: candidate.channel_kind,
                key: candidate.key,
            },
            #[cfg(feature = "metrics")]
            message.bytes_len(),
        );

        let fits = if packet.payload.len() <= mtu {
            true
        } else if compression.is_enabled() {
            matches!(
                evaluate_packet_compression(
                    packet.payload.as_ref(),
                    compression,
                    mtu,
                    compression_scratch,
                    compression_output,
                )?,
                CompressionOutcome::Compressed { .. }
            )
        } else {
            false
        };

        if fits {
            return Ok(true);
        }

        if let Some(previous) = previous_batch {
            packet.payload.as_mut()[previous.count_offset] = previous.count;
        }
        packet.payload.truncate(payload_len);
        packet.messages.truncate(metadata_len);
        *batch = previous_batch;
        Ok(false)
    }

    fn finish_compression_aware_packet(
        &mut self,
        mut packet: Packet,
        compression: CompressionConfig,
        mtu: usize,
        compression_scratch: &mut CompressionScratch,
    ) -> Result<Packet, PacketError> {
        let uncompressed_len = packet.payload.len();
        let outcome = compress_packet(
            &mut packet,
            compression,
            mtu,
            compression_scratch,
            &mut self.compression_output,
        )?;
        match outcome {
            CompressionOutcome::Compressed {
                original_len,
                compressed_len,
            } => {
                Self::trace_compression_outcome(
                    &packet,
                    "compressed",
                    uncompressed_len,
                    Some(original_len),
                    Some(compressed_len),
                );
            }
            CompressionOutcome::NotSmaller {
                original_len,
                compressed_len,
            } => {
                #[cfg(feature = "metrics")]
                metrics::counter!("transport/compression_abandoned").increment(1);
                Self::trace_compression_outcome(
                    &packet,
                    "not_smaller",
                    uncompressed_len,
                    Some(original_len),
                    Some(compressed_len),
                );
                if uncompressed_len > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu,
                    });
                }
            }
            CompressionOutcome::Disabled => {
                Self::trace_compression_outcome(&packet, "disabled", uncompressed_len, None, None);
                if uncompressed_len > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu,
                    });
                }
            }
            CompressionOutcome::AlreadyCompressed => {
                Self::trace_compression_outcome(
                    &packet,
                    "already_compressed",
                    uncompressed_len,
                    None,
                    None,
                );
                if uncompressed_len > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu,
                    });
                }
            }
            CompressionOutcome::TooSmall { payload_len } => {
                Self::trace_compression_outcome(
                    &packet,
                    "too_small",
                    uncompressed_len,
                    Some(payload_len),
                    None,
                );
                if uncompressed_len > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu,
                    });
                }
            }
            CompressionOutcome::TooLargeForDecompressionLimit { payload_len, .. } => {
                Self::trace_compression_outcome(
                    &packet,
                    "too_large_for_decompression_limit",
                    uncompressed_len,
                    Some(payload_len),
                    None,
                );
                if uncompressed_len > mtu {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu,
                    });
                }
            }
        }
        Ok(packet)
    }

    fn trace_compression_outcome(
        packet: &Packet,
        outcome: &'static str,
        packet_len: usize,
        original_len: Option<usize>,
        compressed_len: Option<usize>,
    ) {
        trace!(
            target: "lightyear_debug::transport",
            kind = "packet_compression",
            packet_id = ?packet.packet_id,
            outcome,
            bytes = packet_len,
            original_len = original_len.unwrap_or(0),
            compressed_len = compressed_len.unwrap_or(0),
            num_messages = packet.num_messages(),
            "transport packet compression outcome"
        );
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use core::time::Duration;

    use bevy_app::App;
    use bevy_reflect::TypePath;
    use bevy_utils::default;

    use crate::channel::builder::{ChannelMode, ChannelSettings};
    use crate::channel::fragments::FragmentSender;
    use crate::channel::registry::{AppChannelExt, ChannelKind, ChannelRegistry};
    #[cfg(feature = "compression_lz4")]
    use crate::packet::compression::decompress_payload;
    use crate::packet::error::PacketError;
    #[cfg(feature = "compression_lz4")]
    use crate::packet::header::PacketHeader;
    use crate::packet::message::{
        FragmentCompression, FragmentData, FragmentIndex, MessageId, SendMessage, SendMessageKey,
        SingleData,
    };
    use crate::packet::packet::{FRAGMENT_SIZE, PacketId};
    use bytes::Bytes;

    use super::*;

    #[derive(TypePath)]
    struct Channel1;

    #[derive(TypePath)]
    struct Channel2;

    #[derive(TypePath)]
    struct Channel3;

    fn get_channel_registry() -> ChannelRegistry {
        let mut app = App::new();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        app.init_resource::<ChannelRegistry>();
        app.add_channel::<Channel1>(settings);
        app.add_channel::<Channel2>(settings);
        app.add_channel::<Channel3>(settings);
        app.world_mut()
            .remove_resource::<ChannelRegistry>()
            .unwrap()
    }

    fn single_candidate(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        index: usize,
        bytes: Bytes,
    ) -> SendCandidate {
        SendCandidate::new(
            channel_kind,
            channel_id,
            SendMessageKey::UnreliableSingle(index),
            SendMessage {
                data: SingleData::new(None, bytes).into(),
                priority: 1.0,
            },
        )
    }

    fn fragment_candidates(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        fragments: Vec<FragmentData>,
    ) -> Vec<SendCandidate> {
        fragments
            .into_iter()
            .enumerate()
            .map(|(index, fragment)| {
                SendCandidate::new(
                    channel_kind,
                    channel_id,
                    SendMessageKey::UnreliableFragment(index),
                    SendMessage {
                        data: fragment.into(),
                        priority: 1.0,
                    },
                )
            })
            .collect()
    }

    fn build_staged_packets(
        builder: &mut PacketBuilder,
        candidates: &[SendCandidate],
        compression: CompressionConfig,
    ) -> Result<Vec<Packet>, PacketError> {
        let mut cursor = CandidateCursor::default();
        let mut packets = Vec::new();
        let mut compression_scratch = CompressionScratch::default();
        while let Some(packet) = builder.build_next_packet(
            Tick(0),
            candidates,
            &mut cursor,
            compression,
            DEFAULT_MTU,
            &mut compression_scratch,
        )? {
            builder
                .header_manager
                .commit_send_packet(packet.packet_id, Duration::default());
            packets.push(packet);
        }
        Ok(packets)
    }

    #[cfg(feature = "compression_lz4")]
    fn decompress_packet_for_test(mut packet: Packet) -> Result<Packet, PacketError> {
        let packet_type =
            PacketType::try_from(packet.payload.as_ref()[PacketHeader::PACKET_TYPE_OFFSET])?;
        let decompressed_payload = decompress_payload(
            &packet.payload.as_ref()[HEADER_BYTES..],
            CompressionConfig::LZ4,
        )?;

        packet.payload.as_mut()[PacketHeader::PACKET_TYPE_OFFSET] =
            packet_type.uncompressed_variant().into();
        packet.payload.truncate(HEADER_BYTES);
        packet.payload.extend_from_slice(&decompressed_payload);
        Ok(packet)
    }

    #[cfg(feature = "compression_lz4")]
    fn random_payload(len: usize, message_index: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ message_index as u64;
        let mut payload = Vec::with_capacity(len);
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            payload.push((state & 0xff) as u8);
        }
        payload
    }

    #[test]
    fn buffer_pool_reuses_message_metadata_up_to_retained_capacity() {
        let mut pool = PacketBufferPool::new(DEFAULT_MTU);

        pool.recycle_message_metadata(Vec::with_capacity(MAX_RETAINED_MESSAGE_METADATA_CAPACITY));
        assert_eq!(
            pool.take_message_metadata().capacity(),
            MAX_RETAINED_MESSAGE_METADATA_CAPACITY
        );
    }

    #[test]
    fn buffer_pool_drops_oversized_message_metadata() {
        let mut pool = PacketBufferPool::new(DEFAULT_MTU);

        pool.recycle_message_metadata(Vec::with_capacity(
            MAX_RETAINED_MESSAGE_METADATA_CAPACITY + 1,
        ));
        assert_eq!(pool.take_message_metadata().capacity(), 0);
    }

    #[test]
    fn buffer_pool_does_not_retain_payloads_that_grew_past_the_mtu() {
        let mut pool = PacketBufferPool::new(DEFAULT_MTU);
        let mut payload = pool.take_payload();
        payload.extend_from_slice(&alloc::vec![0; DEFAULT_MTU + 1]);

        pool.payloads.recycle(payload);

        let misses = pool.payload_pool_misses();
        let _ = pool.take_payload();
        assert_eq!(pool.payload_pool_misses(), misses + 1);
    }

    #[test]
    fn send_handoff_rejects_an_oversized_allocation_before_splitting() {
        let mut builder = PacketBuilder::new(1.5);
        let mut payload = BytesMut::with_capacity(DEFAULT_MTU * 2);
        payload.extend_from_slice(&alloc::vec![0; DEFAULT_MTU]);
        let mut packet = Packet {
            payload,
            messages: Vec::new(),
            packet_id: PacketId(0),
            compression: None,
        };

        let bytes = builder.take_send_payload(&mut packet);

        assert_eq!(bytes.len(), DEFAULT_MTU);
        assert_eq!(packet.payload.capacity(), 0);
        let misses = builder.buffer_pool.payload_pool_misses();
        let _ = builder.buffer_pool.take_payload();
        assert_eq!(builder.buffer_pool.payload_pool_misses(), misses + 1);
    }

    #[test]
    fn staged_builder_packs_singles_across_channels() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channels = [
            ChannelKind::of::<Channel1>(),
            ChannelKind::of::<Channel2>(),
            ChannelKind::of::<Channel3>(),
        ]
        .map(|kind| {
            let id = *channel_registry.get_net_from_kind(&kind).unwrap();
            (kind, id)
        });
        let candidates = [
            (channels[0], Bytes::from_static(b"one")),
            (channels[1], Bytes::from_static(b"two")),
            (channels[1], Bytes::from_static(b"three")),
            (channels[2], Bytes::from_static(b"four")),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, ((kind, id), bytes))| single_candidate(kind, id, index, bytes))
        .collect::<Vec<_>>();

        let mut packets = build_staged_packets(
            &mut PacketBuilder::new(1.5),
            &candidates,
            CompressionConfig::DISABLED,
        )?;
        assert_eq!(packets.len(), 1);
        let contents = packets.pop().unwrap().parse_packet_payload()?;
        assert_eq!(contents[&channels[0].1], [Bytes::from_static(b"one")]);
        assert_eq!(
            contents[&channels[1].1],
            [Bytes::from_static(b"two"), Bytes::from_static(b"three")]
        );
        assert_eq!(contents[&channels[2].1], [Bytes::from_static(b"four")]);
        Ok(())
    }

    #[test]
    fn staged_builder_splits_singles_at_mtu() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channels = [
            ChannelKind::of::<Channel1>(),
            ChannelKind::of::<Channel2>(),
            ChannelKind::of::<Channel3>(),
        ]
        .map(|kind| {
            let id = *channel_registry.get_net_from_kind(&kind).unwrap();
            (kind, id)
        });
        let candidates = [channels[0], channels[1], channels[1], channels[2]]
            .into_iter()
            .enumerate()
            .map(|(index, (kind, id))| {
                single_candidate(kind, id, index, Bytes::from(vec![index as u8; 500]))
            })
            .collect::<Vec<_>>();

        let packets = build_staged_packets(
            &mut PacketBuilder::new(1.5),
            &candidates,
            CompressionConfig::DISABLED,
        )?;
        assert_eq!(packets.len(), 2);
        assert!(
            packets
                .iter()
                .all(|packet| packet.payload.len() <= DEFAULT_MTU)
        );
        let message_count = packets
            .into_iter()
            .map(Packet::parse_packet_payload)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|contents| contents.values().map(Vec::len).sum::<usize>())
            .sum::<usize>();
        assert_eq!(message_count, 4);
        Ok(())
    }

    #[test]
    fn staged_fragment_packets_respect_worst_case_mtu_overhead() -> Result<(), PacketError> {
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = 64;
        let fragments = FragmentSender::new().build_fragments(
            MessageId(1024),
            Bytes::from(vec![9u8; FRAGMENT_SIZE * 64 + 1]),
        );
        let expected_packet_count = fragments.len();
        let candidates = fragment_candidates(channel_kind, channel_id, fragments);

        let packets = build_staged_packets(
            &mut PacketBuilder::new(1.5),
            &candidates,
            CompressionConfig::DISABLED,
        )?;
        assert_eq!(packets.len(), expected_packet_count);
        assert!(
            packets
                .iter()
                .all(|packet| packet.payload.len() <= DEFAULT_MTU)
        );
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn staged_compression_can_pack_beyond_uncompressed_mtu() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = *channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let candidates = (0..128)
            .map(|index| {
                single_candidate(channel_kind, channel_id, index, Bytes::from(vec![7u8; 64]))
            })
            .collect::<Vec<_>>();
        let compression = CompressionConfig {
            min_payload_size: 0,
            ..CompressionConfig::LZ4
        };

        let mut packets =
            build_staged_packets(&mut PacketBuilder::new(1.5), &candidates, compression)?;
        assert_eq!(packets.len(), 1);
        let packet = packets.pop().unwrap();
        assert!(packet.payload.len() <= DEFAULT_MTU);
        assert_eq!(
            PacketType::try_from(packet.payload.as_ref()[PacketHeader::PACKET_TYPE_OFFSET])?,
            PacketType::DataCompressed
        );
        let contents = decompress_packet_for_test(packet)?.parse_packet_payload()?;
        assert_eq!(contents[&channel_id].len(), 128);
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn staged_compression_preserves_mtu_for_incompressible_data() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = *channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let candidates = (0..128)
            .map(|index| {
                single_candidate(
                    channel_kind,
                    channel_id,
                    index,
                    random_payload(128, index).into(),
                )
            })
            .collect::<Vec<_>>();
        let compression = CompressionConfig {
            min_payload_size: 0,
            ..CompressionConfig::LZ4
        };

        let packets = build_staged_packets(&mut PacketBuilder::new(1.5), &candidates, compression)?;
        let mut message_count = 0;
        for packet in packets {
            assert!(packet.payload.len() <= DEFAULT_MTU);
            let packet_type =
                PacketType::try_from(packet.payload.as_ref()[PacketHeader::PACKET_TYPE_OFFSET])?;
            let packet = if packet_type.is_compressed() {
                decompress_packet_for_test(packet)?
            } else {
                packet
            };
            message_count += packet
                .parse_packet_payload()?
                .get(&channel_id)
                .map_or(0, Vec::len);
        }
        assert_eq!(message_count, 128);
        Ok(())
    }

    #[test]
    fn staged_builder_fills_each_final_fragment_with_singles() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = *channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let mut candidates = Vec::new();

        for (message_id, key_offset) in [(MessageId(0), 0), (MessageId(1), 2)] {
            for fragment_index in 0..2 {
                candidates.push(SendCandidate::new(
                    channel_kind,
                    channel_id,
                    SendMessageKey::UnreliableFragment(key_offset + fragment_index),
                    SendMessage {
                        data: FragmentData {
                            message_id,
                            fragment_id: FragmentIndex(fragment_index as u64),
                            num_fragments: FragmentIndex(2),
                            compression: (fragment_index == 0).then_some(FragmentCompression::None),
                            bytes: Bytes::from(vec![fragment_index as u8; 900]),
                        }
                        .into(),
                        priority: 1.0,
                    },
                ));
            }
        }
        candidates.extend((0..5).map(|index| {
            SendCandidate::new(
                channel_kind,
                channel_id,
                SendMessageKey::UnreliableSingle(index),
                SendMessage {
                    data: SingleData::new(None, Bytes::from(vec![index as u8; 100])).into(),
                    priority: 1.0,
                },
            )
        }));

        let mut builder = PacketBuilder::new(1.5);
        let mut cursor = CandidateCursor::default();
        let mut packets = Vec::new();
        let mut compression_scratch = CompressionScratch::default();
        while let Some(packet) = builder.build_next_packet(
            Tick(0),
            &candidates,
            &mut cursor,
            CompressionConfig::DISABLED,
            DEFAULT_MTU,
            &mut compression_scratch,
        )? {
            builder
                .header_manager
                .commit_send_packet(packet.packet_id, Duration::default());
            packets.push(packet);
        }

        assert_eq!(packets.len(), 5);
        for packet_index in [1, 3] {
            let packet = &packets[packet_index];
            assert_eq!(packet.messages.len(), 3);
            assert_eq!(packet.messages[0].fragment_index, Some(FragmentIndex(1)));
            assert!(
                packet.messages[1..]
                    .iter()
                    .all(|metadata| metadata.fragment_index.is_none())
            );
        }
        assert_eq!(packets[4].messages.len(), 1);
        assert!(packets[4].messages[0].fragment_index.is_none());
        Ok(())
    }

    #[test]
    fn staged_builder_splits_channel_batches_at_u8_limit() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = *channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let candidates = (0..300)
            .map(|index| {
                SendCandidate::new(
                    channel_kind,
                    channel_id,
                    SendMessageKey::UnreliableSingle(index),
                    SendMessage {
                        data: SingleData::new(None, Bytes::new()).into(),
                        priority: 1.0,
                    },
                )
            })
            .collect::<Vec<_>>();

        let mut builder = PacketBuilder::new(1.5);
        let mut cursor = CandidateCursor::default();
        let mut compression_scratch = CompressionScratch::default();
        let packet = builder
            .build_next_packet(
                Tick(0),
                &candidates,
                &mut cursor,
                CompressionConfig::DISABLED,
                DEFAULT_MTU,
                &mut compression_scratch,
            )?
            .unwrap();
        assert_eq!(packet.messages.len(), 300);
        let packet_id = packet.packet_id;
        let contents = packet.parse_packet_payload()?;
        assert_eq!(contents[&channel_id].len(), 300);
        builder
            .header_manager
            .commit_send_packet(packet_id, Duration::default());
        assert!(
            builder
                .build_next_packet(
                    Tick(0),
                    &candidates,
                    &mut cursor,
                    CompressionConfig::DISABLED,
                    DEFAULT_MTU,
                    &mut compression_scratch,
                )?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn final_fragment_tail_uses_actual_single_channel_width_and_rolls_back()
    -> Result<(), PacketError> {
        let mtu = 128;
        let fragment_channel_id = 0;
        let single_channel_id = 64;
        assert!(single_channel_id.bytes_len() > fragment_channel_id.bytes_len());

        let single = SingleData::new(None, Bytes::new());
        let single_wire_len = single_channel_id.bytes_len() + 1 + single.bytes_len();
        let fragment_len = (0..mtu)
            .rev()
            .find(|fragment_len| {
                let fragment = FragmentData {
                    message_id: MessageId(0),
                    fragment_id: FragmentIndex(0),
                    num_fragments: FragmentIndex(1),
                    compression: Some(FragmentCompression::None),
                    bytes: Bytes::from(vec![0; *fragment_len]),
                };
                let packet_len =
                    HEADER_BYTES + fragment_channel_id.bytes_len() + fragment.bytes_len();
                packet_len <= mtu && mtu - packet_len < single_wire_len
            })
            .expect("test MTU should leave a tail smaller than the single batch");

        let candidates = vec![
            SendCandidate::new(
                ChannelKind::of::<Channel1>(),
                fragment_channel_id,
                SendMessageKey::UnreliableFragment(0),
                SendMessage {
                    data: FragmentData {
                        message_id: MessageId(0),
                        fragment_id: FragmentIndex(0),
                        num_fragments: FragmentIndex(1),
                        compression: Some(FragmentCompression::None),
                        bytes: Bytes::from(vec![0; fragment_len]),
                    }
                    .into(),
                    priority: 1.0,
                },
            ),
            single_candidate(
                ChannelKind::of::<Channel2>(),
                single_channel_id,
                0,
                Bytes::new(),
            ),
        ];

        let mut builder = PacketBuilder::new(1.5);
        let mut cursor = CandidateCursor::default();
        let mut compression_scratch = CompressionScratch::default();
        let fragment_packet = builder
            .build_next_packet(
                Tick(0),
                &candidates,
                &mut cursor,
                CompressionConfig::DISABLED,
                mtu,
                &mut compression_scratch,
            )?
            .unwrap();
        assert!(fragment_packet.payload.len() <= mtu);
        assert_eq!(fragment_packet.messages.len(), 1);
        builder
            .header_manager
            .commit_send_packet(fragment_packet.packet_id, Duration::ZERO);

        let single_packet = builder
            .build_next_packet(
                Tick(0),
                &candidates,
                &mut cursor,
                CompressionConfig::DISABLED,
                mtu,
                &mut compression_scratch,
            )?
            .unwrap();
        assert!(single_packet.payload.len() <= mtu);
        assert_eq!(single_packet.messages.len(), 1);
        assert_eq!(single_packet.messages[0].channel, single_channel_id);
        Ok(())
    }
}
