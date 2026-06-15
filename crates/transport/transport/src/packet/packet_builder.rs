//! Module to take a buffer of messages to send and build packets
use crate::channel::registry::ChannelId;
use crate::packet::compression::{
    CompressionCandidate, CompressionConfig, CompressionOutcome,
    try_build_compressed_packet_payload, try_compress_packet,
};
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::{FragmentData, SingleData};
use crate::packet::packet::{FRAGMENT_SIZE, HEADER_BYTES, Packet};
use crate::packet::packet_type::PacketType;
use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::network::NetId;
use lightyear_core::tick::Tick;
use lightyear_serde::{SerializationError, ToBytes, varint::varint_len, writer::WriteInteger};
use tracing::trace;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};

pub const MAX_PACKET_SIZE: usize = 1200;
pub const MAX_UNFRAGMENTED_PAYLOAD_SIZE: usize = FRAGMENT_SIZE;

pub type Payload = Vec<u8>;

const MAX_MESSAGES_PER_CHANNEL_BATCH: usize = u8::MAX as usize;

/// We use `Bytes` on the receive side because we want to be able to refer to sub-slices of the original
/// packet without allocating.
///
/// e.g. we receive 1200 bytes from the network, we want to read parts of it (header, channel) but then
/// store subslices in receiver channels without allocating.
pub type RecvPayload = Bytes;

/// `PacketBuilder` handles the process of creating a packet (writing the header and packing the
/// messages into packets)
#[derive(Debug)]
pub(crate) struct PacketBuilder {
    pub(crate) header_manager: PacketHeaderManager,
    current_packet: Option<Packet>,
    /// Max size for a single packet
    mtu: usize,
    // Pre-allocated buffer to encode/decode without allocation.
    // TODO: should this be associated with Packet?
    // cursor: Vec<u8>,
    // acks: Vec<(ChannelId, Vec<MessageAck>)>,
    // How many bytes we know we are going to have to write in the packet, but haven't written yet
    // prewritten_size: usize,
    // mid_packet: bool,
}

impl Default for PacketBuilder {
    fn default() -> Self {
        Self::new(1.5)
    }
}

impl PacketBuilder {
    pub fn new(nack_rtt_multiple: f32) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(nack_rtt_multiple),
            current_packet: None,
            mtu: MAX_PACKET_SIZE,
            // cursor: Vec::with_capacity(PACKET_BUFFER_CAPACITY),
            // acks: Vec::new(),

            // we start with 1 extra byte for the final ChannelId = 0 that marks the end of the packet
            // prewritten_size: 0,
            // are we in the middle of writing a packet?
            // mid_packet: false,
        }
    }

    // TODO: get the vec from a pool of preallocated buffers
    fn get_new_buffer(&self) -> Payload {
        Vec::with_capacity(self.mtu)
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_single_packet(
        &mut self,
        real: Duration,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        let mut cursor = self.get_new_buffer();

        // write the header
        let header =
            self.header_manager
                .prepare_send_packet_header(PacketType::Data, real, current_tick);
        header.to_bytes(&mut cursor)?;
        self.current_packet = Some(Packet {
            payload: cursor,
            messages: vec![],
            packet_id: header.packet_id,
            prewritten_size: 0,
            compression: None,
        });
        Ok(())
    }

    pub(crate) fn build_new_fragment_packet(
        &mut self,
        real: Duration,
        channel_id: NetId,
        fragment_data: &FragmentData,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        let mut cursor = self.get_new_buffer();
        // writer the header
        let header = self.header_manager.prepare_send_packet_header(
            PacketType::DataFragment,
            real,
            current_tick,
        );
        header.to_bytes(&mut cursor)?;
        channel_id.to_bytes(&mut cursor)?;
        fragment_data.to_bytes(&mut cursor)?;
        let mut packet = Packet {
            payload: cursor,
            // TODO(perf): reuse this vec allocation instead of newly allocating!
            messages: vec![],
            packet_id: header.packet_id,
            prewritten_size: 0,
            compression: None,
        };
        packet.record_message_metadata(
            ChannelId::from(channel_id),
            Some(fragment_data.message_id),
            Some(fragment_data.fragment_id),
            Some(fragment_data.num_fragments.0),
            #[cfg(feature = "metrics")]
            fragment_data.bytes.len(),
        );
        self.current_packet = Some(packet);
        Ok(())

        //
        // let is_last_fragment = fragment_data.is_last_fragment();
        // let packet = FragmentedPacket::new(channel_id, fragment_data);
        //
        // debug_assert!(packet.fragment.bytes.len() <= FRAGMENT_SIZE);
        // if is_last_fragment {
        //     packet.encode(&mut self.try_write_buffer).unwrap();
        //     // reserve one extra bit for the continuation bit between fragment/single packet data
        //     self.try_write_buffer.reserve_bits(1);
        //
        //     // let num_bits_written = self.try_write_buffer.num_bits_written();
        //     // no need to reserve bits, since we already just wrote in the try buffer!
        //     // self.try_write_buffer.reserve_bits(num_bits_written);
        //     debug_assert!(!self.try_write_buffer.overflowed())
        // }
        //
        // Packet {
        //     header,
        //     data: PacketData::Fragmented(packet),
        // }
    }

    pub fn finish_packet(&mut self) -> Packet {
        Self::finalize_packet(self.current_packet.take().unwrap())
    }

    fn finalize_packet(mut packet: Packet) -> Packet {
        packet.payload.shrink_to_fit();
        packet
    }

    /// Pack messages into packets
    ///
    /// In general the strategy is:
    /// - sort the single data messages from smallest to largest
    /// - write the fragment data first. Big fragments take the entire packet. Small fragments have
    ///   some room to spare for small messages
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn build_packets(
        &mut self,
        real: Duration,
        current_tick: Tick,
        single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
        fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)>,
    ) -> Result<Vec<Packet>, SerializationError> {
        self.build_packets_uncompressed(real, current_tick, single_data, fragment_data)
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn build_packets_with_compression(
        &mut self,
        real: Duration,
        current_tick: Tick,
        single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
        fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)>,
        compression: CompressionConfig,
    ) -> Result<Vec<Packet>, PacketError> {
        if !compression.is_enabled() {
            let packets =
                self.build_packets_uncompressed(real, current_tick, single_data, fragment_data)?;
            for packet in &packets {
                Self::trace_compression_outcome(
                    packet,
                    "disabled",
                    packet.payload.len(),
                    None,
                    None,
                );
            }
            return Ok(packets);
        }
        self.build_packets_internal(real, current_tick, single_data, fragment_data, compression)
    }

    fn build_packets_uncompressed(
        &mut self,
        real: Duration,
        current_tick: Tick,
        single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
        fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)>,
    ) -> Result<Vec<Packet>, SerializationError> {
        self.build_packets_internal(
            real,
            current_tick,
            single_data,
            fragment_data,
            CompressionConfig::DISABLED,
        )
        .map_err(|error| match error {
            PacketError::Serialization(error) => error,
            other => panic!("unexpected packet builder error without compression: {other:?}"),
        })
    }

    fn build_packets_internal(
        &mut self,
        real: Duration,
        current_tick: Tick,
        mut single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
        fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)>,
        compression: CompressionConfig,
    ) -> Result<Vec<Packet>, PacketError> {
        let mut packets: Vec<Packet> = vec![];

        // indices in the main vec
        let mut single_data_idx = 0;

        for (_, single_messages) in single_data.iter_mut() {
            // sort from smallest to largest each array of small messages
            single_messages
                .make_contiguous()
                .sort_by_key(|message| message.bytes.len());
        }

        // try to fill the packet with fragment messages first
        for (channel_id, mut fragment_messages) in fragment_data.into_iter() {
            while let Some(fragment_data) = fragment_messages.pop_front() {
                debug_assert!(fragment_data.bytes.len() <= FRAGMENT_SIZE);
                self.build_new_fragment_packet(real, channel_id, &fragment_data, current_tick)?;
                if !fragment_data.is_last_fragment() {
                    // big fragment, write packet immediately
                    packets.push(self.finish_packet());
                } else {
                    let mut packet = self.current_packet.take().unwrap();
                    // it's a smaller fragment, fill it with small messages
                    'out: while single_data_idx < single_data.len() {
                        // if we don't even have space for a new channel, return the packet immediately
                        if !packet.can_fit_channel(channel_id) {
                            break;
                        }

                        let (channel_id, single_messages) = &mut single_data[single_data_idx];
                        // number of messages for this channel that we will write
                        // (we wait until we know the full number, because we want to write that)
                        let mut num_messages = 0;
                        // fill with messages from the current channel
                        loop {
                            // no more messages to send in this channel, try to fill with messages from the next channels
                            if num_messages == single_messages.len() {
                                Self::write_single_messages(
                                    &mut packet,
                                    single_messages,
                                    &mut num_messages,
                                    *channel_id,
                                )?;
                                single_data_idx += 1;
                                break;
                            }

                            if packet.can_fit(single_messages[num_messages].bytes_len()) {
                                packet.prewritten_size += single_messages[num_messages].bytes_len();
                                num_messages += 1;
                            } else {
                                // can't add any more messages (since we sorted messages from smallest to largest)
                                // finish packet and go back to trying to write fragment messages
                                Self::write_single_messages(
                                    &mut packet,
                                    single_messages,
                                    &mut num_messages,
                                    *channel_id,
                                )?;
                                break 'out;
                            }
                        }
                    }
                    // no more single messages to send, finish the fragment packet
                    self.current_packet = Some(packet);
                    packets.push(self.finish_packet());
                }
            }
        }

        debug_assert!(self.current_packet.is_none());

        // all fragment messages have been written, now write small messages
        if compression.is_enabled() {
            self.build_single_packets_compression_aware(
                real,
                current_tick,
                &mut single_data,
                single_data_idx,
                &mut packets,
                compression,
            )?;
            return Ok(packets);
        }

        'out: while single_data_idx < single_data.len() {
            let (channel_id, single_messages) = &mut single_data[single_data_idx];
            // start a new packet if we aren't already writing one
            if self.current_packet.is_none() {
                self.build_new_single_packet(real, current_tick)?;
            }

            let mut packet = self.current_packet.take().unwrap();
            // we need to call this to preassign the channel_id
            if !packet.can_fit_channel(*channel_id) {
                // can't add any more messages (since we sorted messages from smallest to largest)
                // finish packet and go back to trying to write fragment messages
                self.current_packet = Some(packet);
                packets.push(self.finish_packet());
                continue 'out;
            }
            // number of messages for this channel that we will write
            // (we wait until we know the full number, because we want to write that)
            let mut num_messages = 0;
            // fill with messages from the current channel
            loop {
                // no more messages to send in this channel, try to fill with messages from the next channels
                if num_messages == single_messages.len() {
                    Self::write_single_messages(
                        &mut packet,
                        single_messages,
                        &mut num_messages,
                        *channel_id,
                    )?;
                    // we make sure we keep writing the current packet
                    self.current_packet = Some(packet);
                    single_data_idx += 1;
                    break;
                }

                if packet.can_fit(single_messages[num_messages].bytes_len()) {
                    packet.prewritten_size += single_messages[num_messages].bytes_len();
                    num_messages += 1;
                } else {
                    // can't add any more messages (since we sorted messages from smallest to largest)
                    // finish packet and go back to trying to write fragment messages
                    Self::write_single_messages(
                        &mut packet,
                        single_messages,
                        &mut num_messages,
                        *channel_id,
                    )?;
                    self.current_packet = Some(packet);
                    packets.push(self.finish_packet());
                    continue 'out;
                }
            }
        }

        // if we had a packet we were working on, push it
        if self.current_packet.is_some() {
            packets.push(self.finish_packet());
        }
        Ok(packets)
    }

    /// Helper function to fill the current packet with single data message from the current channel
    fn write_single_messages(
        packet: &mut Packet,
        messages: &mut VecDeque<SingleData>,
        num_messages: &mut usize,
        channel_id: ChannelId,
    ) -> Result<(), SerializationError> {
        packet.prewritten_size = packet
            .prewritten_size
            .checked_sub(varint_len(channel_id as u64) + 1)
            .ok_or(SerializationError::SubtractionOverflow)?;
        if *num_messages > 0 {
            trace!("Writing packet with {} messages", *num_messages);
            channel_id.to_bytes(&mut packet.payload)?;
            // write the number of messages for the current channel
            packet.payload.write_u8(*num_messages as u8)?;
            // write the messages
            for _ in 0..*num_messages {
                // TODO: deal with error
                let message = messages.pop_front().unwrap();
                #[cfg(feature = "metrics")]
                let message_bytes = message.bytes_len();
                message.to_bytes(&mut packet.payload)?;
                packet.prewritten_size = packet
                    .prewritten_size
                    .checked_sub(message.bytes_len())
                    .ok_or(SerializationError::SubtractionOverflow)?;
                packet.record_message_metadata(
                    channel_id,
                    message.id,
                    None,
                    None,
                    #[cfg(feature = "metrics")]
                    message_bytes,
                );
            }
            *num_messages = 0;
        }
        Ok(())
    }

    fn build_single_packets_compression_aware(
        &mut self,
        real: Duration,
        current_tick: Tick,
        single_data: &mut [(ChannelId, VecDeque<SingleData>)],
        mut single_data_idx: usize,
        packets: &mut Vec<Packet>,
        compression: CompressionConfig,
    ) -> Result<(), PacketError> {
        while single_data_idx < single_data.len() {
            if single_data[single_data_idx].1.is_empty() {
                single_data_idx += 1;
                continue;
            }

            if self.current_packet.is_none() {
                self.build_new_single_packet(real, current_tick)?;
            }

            let mut packet = self.current_packet.take().unwrap();
            let (channel_id, single_messages) = &mut single_data[single_data_idx];
            let max_count = single_messages.len().min(MAX_MESSAGES_PER_CHANNEL_BATCH);
            let fit_count = Self::find_compression_aware_message_count(
                &packet,
                *channel_id,
                single_messages,
                max_count,
                compression,
            )?;

            if fit_count == 0 {
                if Self::packet_has_body(&packet) {
                    packets.push(Self::finish_compression_aware_packet(packet, compression)?);
                    continue;
                }
                let candidate_len =
                    Self::candidate_packet_len(&packet, *channel_id, single_messages, 1)?;
                return Err(PacketError::PacketTooLarge {
                    actual: candidate_len,
                    mtu: MAX_PACKET_SIZE,
                });
            }

            Self::append_single_messages(
                &mut packet,
                *channel_id,
                single_messages,
                fit_count,
                true,
            )?;
            for _ in 0..fit_count {
                single_messages.pop_front();
            }

            let channel_drained = single_messages.is_empty();
            let hit_channel_batch_limit =
                fit_count == MAX_MESSAGES_PER_CHANNEL_BATCH && !channel_drained;
            let packet_full = fit_count < max_count || hit_channel_batch_limit;

            if channel_drained {
                single_data_idx += 1;
            }

            if packet_full {
                packets.push(Self::finish_compression_aware_packet(packet, compression)?);
            } else {
                self.current_packet = Some(packet);
            }
        }

        if let Some(packet) = self.current_packet.take()
            && Self::packet_has_body(&packet)
        {
            packets.push(Self::finish_compression_aware_packet(packet, compression)?);
        }
        Ok(())
    }

    fn find_compression_aware_message_count(
        packet: &Packet,
        channel_id: ChannelId,
        messages: &VecDeque<SingleData>,
        max_count: usize,
        compression: CompressionConfig,
    ) -> Result<usize, PacketError> {
        if max_count == 0 {
            return Ok(0);
        }

        if !Self::candidate_packet_fits(packet, channel_id, messages, 1, compression)? {
            return Ok(0);
        }

        let mut best = 1;
        let mut next = 2;

        while next <= max_count {
            if Self::candidate_packet_fits(packet, channel_id, messages, next, compression)? {
                best = next;
                if next == max_count {
                    return Ok(best);
                }
                next = next.saturating_mul(2).min(max_count);
            } else {
                break;
            }
        }

        let mut low = best + 1;
        let mut high = next.min(max_count);
        while low <= high {
            let mid = low + (high - low) / 2;
            if Self::candidate_packet_fits(packet, channel_id, messages, mid, compression)? {
                best = mid;
                low = mid + 1;
            } else if mid == 0 {
                break;
            } else {
                high = mid - 1;
            }
        }

        Ok(best)
    }

    fn candidate_packet_fits(
        packet: &Packet,
        channel_id: ChannelId,
        messages: &VecDeque<SingleData>,
        count: usize,
        compression: CompressionConfig,
    ) -> Result<bool, PacketError> {
        let candidate = Self::candidate_packet(packet, channel_id, messages, count)?;
        let uncompressed_fits = candidate.payload.len() <= MAX_PACKET_SIZE;
        let compression_candidate =
            try_build_compressed_packet_payload(&candidate.payload, compression)?;
        #[cfg(feature = "metrics")]
        if matches!(
            compression_candidate,
            CompressionCandidate::Compressed { .. } | CompressionCandidate::NotSmaller { .. }
        ) {
            metrics::counter!("transport/compression_abandoned").increment(1);
        }
        Ok(uncompressed_fits
            || matches!(
                compression_candidate,
                CompressionCandidate::Compressed { .. }
            ))
    }

    fn candidate_packet_len(
        packet: &Packet,
        channel_id: ChannelId,
        messages: &VecDeque<SingleData>,
        count: usize,
    ) -> Result<usize, SerializationError> {
        Ok(Self::candidate_packet(packet, channel_id, messages, count)?
            .payload
            .len())
    }

    fn candidate_packet(
        packet: &Packet,
        channel_id: ChannelId,
        messages: &VecDeque<SingleData>,
        count: usize,
    ) -> Result<Packet, SerializationError> {
        let mut candidate = Packet {
            payload: packet.payload.clone(),
            messages: Vec::new(),
            packet_id: packet.packet_id,
            prewritten_size: 0,
            compression: None,
        };
        Self::append_single_messages(&mut candidate, channel_id, messages, count, false)?;
        Ok(candidate)
    }

    fn append_single_messages(
        packet: &mut Packet,
        channel_id: ChannelId,
        messages: &VecDeque<SingleData>,
        count: usize,
        record_metadata: bool,
    ) -> Result<(), SerializationError> {
        debug_assert!(count <= MAX_MESSAGES_PER_CHANNEL_BATCH);
        channel_id.to_bytes(&mut packet.payload)?;
        packet.payload.write_u8(count as u8)?;
        for message in messages.iter().take(count) {
            #[cfg(feature = "metrics")]
            let message_bytes = message.bytes_len();
            message.to_bytes(&mut packet.payload)?;
            if record_metadata {
                packet.record_message_metadata(
                    channel_id,
                    message.id,
                    None,
                    None,
                    #[cfg(feature = "metrics")]
                    message_bytes,
                );
            }
        }
        Ok(())
    }

    fn finish_compression_aware_packet(
        mut packet: Packet,
        compression: CompressionConfig,
    ) -> Result<Packet, PacketError> {
        let uncompressed_len = packet.payload.len();
        let outcome = try_compress_packet(&mut packet, compression)?;
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
                if uncompressed_len > MAX_PACKET_SIZE {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu: MAX_PACKET_SIZE,
                    });
                }
            }
            CompressionOutcome::Disabled => {
                Self::trace_compression_outcome(&packet, "disabled", uncompressed_len, None, None);
                if uncompressed_len > MAX_PACKET_SIZE {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu: MAX_PACKET_SIZE,
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
                if uncompressed_len > MAX_PACKET_SIZE {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu: MAX_PACKET_SIZE,
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
                if uncompressed_len > MAX_PACKET_SIZE {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu: MAX_PACKET_SIZE,
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
                if uncompressed_len > MAX_PACKET_SIZE {
                    return Err(PacketError::PacketTooLarge {
                        actual: uncompressed_len,
                        mtu: MAX_PACKET_SIZE,
                    });
                }
            }
        }
        Ok(Self::finalize_packet(packet))
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

    fn packet_has_body(packet: &Packet) -> bool {
        packet.payload.len() > HEADER_BYTES
    }

    // /// Uses multiple exponential searches to fill a packet. Has a good worst case runtime and doesn't
    // /// create any extraneous extension packets.
    // fn pack_multiple_exponential(mut messages: &[Message]) -> Vec<Packet> {
    //     /// A Vec<u8> prefixed by its length as a u32. Each [`Packet`] contains 1 or more [`Section`]s.
    //     struct Section(Vec<u8>);
    //     impl Section {
    //         fn len(&self) -> usize {
    //             self.0.len() + core::mem::size_of::<u32>()
    //         }
    //         fn write(&self, out: &mut Vec<u8>) {
    //             out.reserve(self.len());
    //             out.extend_from_slice(&u32::try_from(self.0.len()).unwrap().to_le_bytes()); // TODO use varint.
    //             out.extend_from_slice(&self.0);
    //         }
    //     }
    //
    //     let mut buffer = bitcode::Buffer::new(); // TODO save between calls.
    //     let mut packets = vec![];
    //
    //     while !messages.is_empty() {
    //         let mut remaining = Packet::MAX_SIZE;
    //         let mut bytes = vec![];
    //
    //         while remaining > 0 && !messages.is_empty() {
    //             let mut i = 0;
    //             let mut previous = None;
    //
    //             loop {
    //                 i = (i * 2).clamp(1, messages.len());
    //                 const COMPRESS: bool = true;
    //                 let b = Section(if COMPRESS {
    //                     lz4_flex::compress_prepend_size(&buffer.encode(&messages[..i]))
    //                 } else {
    //                     buffer.encode(&messages[..i]).to_vec()
    //                 });
    //
    //                 let (i, b) = if b.len() <= remaining {
    //                     if i == messages.len() {
    //                         // No more messages.
    //                         (i, b)
    //                     } else {
    //                         // Try to fit more.
    //                         previous = Some((i, b));
    //                         continue;
    //                     }
    //                 } else if let Some((i, b)) = previous {
    //                     // Current failed, so use previous.
    //                     (i, b)
    //                 } else {
    //                     assert_eq!(i, 1);
    //                     // 1 message doesn't fit. If starting a new packet would result in fewer
    //                     // fragments, flush the current packet.
    //                     let flush_fragments = b.len().div_ceil(Packet::MAX_SIZE) - 1;
    //                     let keep_fragments = (b.len() - remaining).div_ceil(Packet::MAX_SIZE);
    //                     if flush_fragments < keep_fragments {
    //                         // TODO try to fill current packet by with packets after the single large packet.
    //                         packets.push(Packet(core::mem::take(&mut bytes)));
    //                         remaining = Packet::MAX_SIZE;
    //                     }
    //                     (i, b)
    //                 };
    //
    //                 messages = &messages[i..];
    //                 if bytes.is_empty() && b.len() < Packet::MAX_SIZE {
    //                     bytes = Vec::with_capacity(Packet::MAX_SIZE); // Assume we'll fill the packet.
    //                 }
    //                 b.write(&mut bytes);
    //                 if b.len() > remaining {
    //                     assert_eq!(i, 1);
    //                     // TODO fill extension packets. We would need to know where the section ends
    //                     // within the packet in case previous packets are lost.
    //                     remaining = 0;
    //                 } else {
    //                     remaining -= b.len();
    //                 }
    //                 break;
    //             }
    //         }
    //         packets.push(Packet(bytes));
    //     }
    //     packets
    // }
}

#[cfg(test)]
mod tests {
    use alloc::collections::VecDeque;
    use bevy_app::App;
    use bevy_reflect::TypePath;
    use bevy_utils::default;

    use crate::channel::builder::{ChannelMode, ChannelSettings};
    use crate::channel::registry::{AppChannelExt, ChannelKind, ChannelRegistry};
    use crate::channel::senders::fragment_sender::FragmentSender;
    #[cfg(feature = "compression_lz4")]
    use crate::packet::compression::{CompressionConfig, decompress_payload, try_compress_packet};
    use crate::packet::error::PacketError;
    #[cfg(feature = "compression_lz4")]
    use crate::packet::header::PacketHeader;
    #[cfg(feature = "compression_lz4")]
    use crate::packet::message::FragmentCompression;
    use crate::packet::message::{FragmentIndex, MessageId};
    #[cfg(feature = "compression_lz4")]
    use crate::packet::packet::HEADER_BYTES;
    use crate::packet::packet::MessageMetadata;
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

    #[cfg(feature = "compression_lz4")]
    fn decompress_packet_for_test(mut packet: Packet) -> Result<Packet, PacketError> {
        let packet_type = PacketType::try_from(packet.payload[PacketHeader::PACKET_TYPE_OFFSET])?;
        let decompressed_payload =
            decompress_payload(&packet.payload[HEADER_BYTES..], CompressionConfig::LZ4)?;

        packet.payload[PacketHeader::PACKET_TYPE_OFFSET] =
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

    /// A bunch of small messages that all fit in the same packet
    #[test]
    fn test_pack_small_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![];
        let mut packets =
            manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 1);
        let packet = packets.pop().unwrap();
        #[cfg(not(feature = "metrics"))]
        assert_eq!(packet.messages, vec![]);
        #[cfg(feature = "metrics")]
        assert_eq!(
            packet.messages,
            vec![
                MessageMetadata {
                    channel: *channel_id1,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id2,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id2,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id3,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
            ]
        );
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id1).unwrap(),
            &vec![small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![small_bytes.clone(), small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id3).unwrap(),
            &vec![small_bytes.clone()]
        );
        Ok(())
    }

    /// We cannot write the channel id of the next channel in the packet, so we need to finish the current
    /// packet and start a new one.
    /// The message fills the packet after the current header and message encoding overhead,
    /// leaving no space for another message or channel.
    ///
    /// Test both with different channels and same channels
    #[test]
    fn test_pack_cannot_write_channel_id() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 1178]);
        let small_message = SingleData::new(None, small_bytes.clone());

        {
            let single_data = vec![(
                *channel_id1,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            )];
            let fragment_data = vec![];
            let packets =
                manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
            assert_eq!(packets.len(), 2);
        }
        {
            let single_data = vec![
                (*channel_id1, VecDeque::from(vec![small_message.clone()])),
                (*channel_id2, VecDeque::from(vec![small_message.clone()])),
            ];
            let fragment_data = vec![];
            let packets =
                manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
            assert_eq!(packets.len(), 2);
        }
        Ok(())
    }

    /// A bunch of small messages that all fit in the same packet
    #[test]
    fn test_pack_many_small_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (
                *channel_id1,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
            (
                *channel_id3,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
        ];
        let fragment_data = vec![];
        let packets =
            manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 7);
        Ok(())
    }

    #[test]
    fn compression_candidate_packets_do_not_record_message_metadata() {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let messages = VecDeque::from(vec![
            SingleData::new(Some(MessageId(99)), Bytes::from(vec![7u8; 32])),
            SingleData::new(None, Bytes::from(vec![8u8; 32])),
        ]);

        let mut manager = PacketBuilder::new(1.5);
        manager
            .build_new_single_packet(Duration::default(), Tick(0))
            .unwrap();
        let packet = manager.current_packet.take().unwrap();
        let candidate = PacketBuilder::candidate_packet(&packet, *channel_id, &messages, 2)
            .expect("candidate packet should serialize");

        assert!(candidate.messages.is_empty());
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compression_aware_packing_compresses_under_mtu_packet() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let bytes = Bytes::from(vec![7u8; 512]);
        let single_message = SingleData::new(None, bytes.clone());

        let uncompressed_packets = PacketBuilder::new(1.5).build_packets(
            Duration::default(),
            Tick(0),
            vec![(*channel_id, VecDeque::from(vec![single_message.clone()]))],
            vec![],
        )?;
        assert_eq!(uncompressed_packets.len(), 1);
        assert!(uncompressed_packets[0].payload.len() < MAX_PACKET_SIZE);

        let mut packets = PacketBuilder::new(1.5).build_packets_with_compression(
            Duration::default(),
            Tick(0),
            vec![(*channel_id, VecDeque::from(vec![single_message]))],
            vec![],
            CompressionConfig {
                min_payload_size: 0,
                ..CompressionConfig::LZ4
            },
        )?;

        assert_eq!(packets.len(), 1);
        let packet = packets.pop().unwrap();
        let compression_info = packet
            .compression
            .expect("under-MTU packet should still be compressed");
        assert_eq!(
            PacketType::try_from(packet.payload[PacketHeader::PACKET_TYPE_OFFSET])?,
            PacketType::DataCompressed
        );
        assert!(compression_info.original_len < MAX_PACKET_SIZE);
        assert!(compression_info.compressed_len < compression_info.original_len);
        assert_eq!(packet.payload.len(), compression_info.compressed_len);

        let packet = decompress_packet_for_test(packet)?;
        let contents = packet.parse_packet_payload()?;
        assert_eq!(contents.get(channel_id).unwrap(), &vec![bytes]);
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compression_aware_packing_can_pack_beyond_uncompressed_mtu() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let small_bytes = Bytes::from(vec![7u8; 64]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let uncompressed_packets = PacketBuilder::new(1.5).build_packets(
            Duration::default(),
            Tick(0),
            vec![(
                *channel_id,
                VecDeque::from(vec![small_message.clone(); 128]),
            )],
            vec![],
        )?;
        assert!(uncompressed_packets.len() > 1);

        let mut manager = PacketBuilder::new(1.5);
        let mut packets = manager.build_packets_with_compression(
            Duration::default(),
            Tick(0),
            vec![(
                *channel_id,
                VecDeque::from(vec![small_message.clone(); 128]),
            )],
            vec![],
            CompressionConfig {
                min_payload_size: 0,
                ..CompressionConfig::LZ4
            },
        )?;

        assert_eq!(packets.len(), 1);
        assert!(packets[0].payload.len() <= MAX_PACKET_SIZE);
        #[cfg(feature = "metrics")]
        {
            assert_eq!(packets[0].messages.len(), 128);
            assert!(packets[0].messages.iter().all(|metadata| {
                metadata.channel == *channel_id
                    && metadata.message.is_none()
                    && metadata.fragment_index.is_none()
                    && metadata.num_fragments.is_none()
                    && metadata.num_bytes == small_message.bytes_len()
            }));
        }
        assert_eq!(
            PacketType::try_from(packets[0].payload[PacketHeader::PACKET_TYPE_OFFSET])?,
            PacketType::DataCompressed
        );
        let packet = decompress_packet_for_test(packets.pop().unwrap())?;
        let contents = packet.parse_packet_payload()?;
        assert_eq!(contents.get(channel_id).unwrap().len(), 128);
        assert_eq!(contents.get(channel_id).unwrap()[0], small_bytes);
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compression_aware_packing_preserves_mtu_for_incompressible_data() -> Result<(), PacketError>
    {
        let channel_registry = get_channel_registry();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let messages = (0..128)
            .map(|message_index| SingleData::new(None, random_payload(128, message_index).into()))
            .collect();

        let mut manager = PacketBuilder::new(1.5);
        let packets = manager.build_packets_with_compression(
            Duration::default(),
            Tick(0),
            vec![(*channel_id, messages)],
            vec![],
            CompressionConfig {
                min_payload_size: 0,
                ..CompressionConfig::LZ4
            },
        )?;

        let mut total_messages = 0;
        for packet in packets {
            assert!(packet.payload.len() <= MAX_PACKET_SIZE);
            let packet_type =
                PacketType::try_from(packet.payload[PacketHeader::PACKET_TYPE_OFFSET])?;
            let packet = if packet_type.is_compressed() {
                decompress_packet_for_test(packet)?
            } else {
                packet
            };
            let contents = packet.parse_packet_payload()?;
            total_messages += contents.get(channel_id).map_or(0, Vec::len);
        }
        assert_eq!(total_messages, 128);
        Ok(())
    }

    /// A bunch of small messages that fit in multiple packets
    #[test]
    fn test_pack_single_data_multiple_packets() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 500]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![];
        let packets =
            manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 2);
        Ok(())
    }

    /// Channel 1: small_message
    /// Channel 2: 2 small messages, 1 big fragment, 1 small fragment
    /// Channel 3: small message
    ///
    /// We should get 2 packets: 1 with the 1st big fragment, and 1 packet with the small fragment and all the small messages
    #[test]
    fn test_pack_big_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let num_big_bytes = (1.5 * FRAGMENT_SIZE as f32) as usize;
        let big_bytes = Bytes::from(vec![1u8; num_big_bytes]);
        let fragmenter = FragmentSender::new();
        let fragments = fragmenter.build_fragments(MessageId(3), big_bytes.clone());

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![(*channel_id2, fragments.clone().into())];
        let packets =
            manager.build_packets(Duration::default(), Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 2);

        let mut packets_queue: VecDeque<_> = packets.into();
        // 1st packet
        let packet = packets_queue.pop_front().unwrap();
        assert_eq!(
            packet.messages,
            vec![MessageMetadata {
                channel: *channel_id2,
                message: Some(MessageId(3)),
                fragment_index: Some(FragmentIndex(0)),
                num_fragments: Some(fragments.len() as u64),
                #[cfg(feature = "metrics")]
                num_bytes: fragments[0].bytes.len(),
            }]
        );
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![fragments[0].bytes.clone()]
        );

        // 2nd packet
        let packet = packets_queue.pop_front().unwrap();
        #[cfg(not(feature = "metrics"))]
        assert_eq!(
            packet.messages,
            vec![MessageMetadata {
                channel: *channel_id2,
                message: Some(MessageId(3)),
                fragment_index: Some(FragmentIndex(1)),
                num_fragments: Some(fragments.len() as u64),
                #[cfg(feature = "metrics")]
                num_bytes: fragments[1].bytes.len(),
            }]
        );
        #[cfg(feature = "metrics")]
        assert_eq!(
            packet.messages,
            vec![
                MessageMetadata {
                    channel: *channel_id2,
                    message: Some(MessageId(3)),
                    fragment_index: Some(FragmentIndex(1)),
                    num_fragments: Some(fragments.len() as u64),
                    num_bytes: fragments[1].bytes.len(),
                },
                MessageMetadata {
                    channel: *channel_id1,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id2,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id2,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
                MessageMetadata {
                    channel: *channel_id3,
                    message: None,
                    fragment_index: None,
                    num_fragments: None,
                    num_bytes: small_message.bytes_len(),
                },
            ]
        );
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id1).unwrap(),
            &vec![small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![
                fragments[1].bytes.clone(),
                small_bytes.clone(),
                small_bytes.clone()
            ]
        );
        assert_eq!(
            contents.get(channel_id3).unwrap(),
            &vec![small_bytes.clone()]
        );
        Ok(())
    }

    #[test]
    fn fragmented_packets_never_exceed_max_packet_size() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let bytes = Bytes::from(vec![9u8; FRAGMENT_SIZE + 1]);
        let fragments = FragmentSender::new().build_fragments(MessageId(1024), bytes);

        let packets = manager.build_packets(
            Duration::default(),
            Tick(0),
            vec![],
            vec![(*channel_id, VecDeque::from(fragments))],
        )?;

        assert!(!packets.is_empty());
        for packet in packets {
            assert!(
                packet.payload.len() <= MAX_PACKET_SIZE,
                "fragment packet exceeded MTU: {} > {}",
                packet.payload.len(),
                MAX_PACKET_SIZE
            );
        }
        Ok(())
    }

    #[test]
    fn fragmented_packets_fit_with_two_byte_channel_id() -> Result<(), PacketError> {
        let mut manager = PacketBuilder::new(1.5);
        let bytes = Bytes::from(vec![9u8; FRAGMENT_SIZE + 1]);
        let fragments = FragmentSender::new().build_fragments(MessageId(1024), bytes);

        let packets = manager.build_packets(
            Duration::default(),
            Tick(0),
            vec![],
            vec![(64, VecDeque::from(fragments))],
        )?;

        assert!(!packets.is_empty());
        for packet in packets {
            assert!(
                packet.payload.len() <= MAX_PACKET_SIZE,
                "fragment packet exceeded MTU: {} > {}",
                packet.payload.len(),
                MAX_PACKET_SIZE
            );
        }
        Ok(())
    }

    #[test]
    fn fragmented_packets_fit_with_many_fragments() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let bytes = Bytes::from(vec![9u8; FRAGMENT_SIZE * 64 + 1]);
        let fragments = FragmentSender::new().build_fragments(MessageId(1024), bytes);

        let packets = manager.build_packets(
            Duration::default(),
            Tick(0),
            vec![],
            vec![(*channel_id, VecDeque::from(fragments))],
        )?;

        assert!(!packets.is_empty());
        for packet in packets {
            assert!(
                packet.payload.len() <= MAX_PACKET_SIZE,
                "fragment packet exceeded MTU: {} > {}",
                packet.payload.len(),
                MAX_PACKET_SIZE
            );
        }
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compressed_single_packet_decompresses_to_original_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let bytes = Bytes::from(vec![4u8; 512]);
        let single_data = vec![(
            *channel_id,
            VecDeque::from(vec![SingleData::new(None, bytes.clone())]),
        )];
        let mut packets =
            manager.build_packets(Duration::default(), Tick(0), single_data, vec![])?;
        assert_eq!(packets.len(), 1);

        let mut packet = packets.pop().unwrap();
        try_compress_packet(
            &mut packet,
            CompressionConfig {
                min_payload_size: 0,
                ..CompressionConfig::LZ4
            },
        )?;

        let contents = decompress_packet_for_test(packet)?.parse_packet_payload()?;
        assert_eq!(contents.get(channel_id).unwrap(), &vec![bytes]);
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compressed_fragment_packet_decompresses_to_original_fragment() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let fragment = FragmentData {
            message_id: MessageId(7),
            fragment_id: FragmentIndex(0),
            num_fragments: FragmentIndex(1),
            compression: Some(FragmentCompression::None),
            bytes: Bytes::from(vec![2u8; 512]),
        };
        let mut packets = manager.build_packets(
            Duration::default(),
            Tick(0),
            vec![],
            vec![(*channel_id, VecDeque::from(vec![fragment.clone()]))],
        )?;
        assert_eq!(packets.len(), 1);

        let mut packet = packets.pop().unwrap();
        try_compress_packet(
            &mut packet,
            CompressionConfig {
                min_payload_size: 0,
                ..CompressionConfig::LZ4
            },
        )?;

        let contents = decompress_packet_for_test(packet)?.parse_packet_payload()?;
        assert_eq!(contents.get(channel_id).unwrap(), &vec![fragment.bytes]);
        Ok(())
    }

    // TODO: ADD MORE TESTS
}
