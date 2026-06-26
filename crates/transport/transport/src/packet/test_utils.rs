use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use core::convert::TryInto;
use core::time::Duration;

use bytes::Bytes;
use lightyear_core::prelude::Tick;
use lightyear_link::LinkStats;
use lightyear_serde::SerializationError;
use lightyear_serde::varint::varint_parse_len;

use crate::channel::registry::ChannelId;
use crate::packet::compression::CompressionConfig;
use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::message::SingleData;
use crate::packet::packet_builder::PacketBuilder;
use crate::packet::packet_type::PacketType;

/// Opaque packet-builder input prepared outside allocation measurements.
pub struct PacketLoopBatch {
    single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
}

/// Summary of a completed packet build and parse pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacketLoopStats {
    pub packets: usize,
    pub messages: usize,
    pub payload_bytes: usize,
}

struct SlicePacketReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> SlicePacketReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn has_remaining(&self) -> bool {
        self.position < self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], SerializationError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(SerializationError::InvalidValue)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(SerializationError::InvalidValue)?;
        self.position = end;
        Ok(bytes)
    }

    fn skip(&mut self, len: usize) -> Result<(), SerializationError> {
        self.take(len).map(drop)
    }

    fn read_u8(&mut self) -> Result<u8, SerializationError> {
        Ok(self.take(1)?[0])
    }

    fn read_varint(&mut self) -> Result<u64, SerializationError> {
        let first = *self
            .bytes
            .get(self.position)
            .ok_or(SerializationError::InvalidValue)?;
        let len = varint_parse_len(first);
        let bytes = self.take(len)?;
        match len {
            1 => Ok(u64::from(bytes[0])),
            2 => Ok(u64::from(
                u16::from_be_bytes(bytes.try_into().unwrap()) & 0x3fff,
            )),
            4 => Ok(u64::from(
                u32::from_be_bytes(bytes.try_into().unwrap()) & 0x3fff_ffff,
            )),
            8 => Ok(u64::from_be_bytes(bytes.try_into().unwrap()) & 0x3fff_ffff_ffff_ffff),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}

/// Focused test fixture for the packet send/receive loop.
///
/// This intentionally bypasses Bevy schedules, typed message serialization, IO, and connection
/// layers. Call [`prepare_batch`](Self::prepare_batch) before starting an allocation measurement,
/// then call [`run_batch`](Self::run_batch) inside the measured region.
pub struct PacketLoopFixture {
    packet_builder: PacketBuilder,
    channel_id: ChannelId,
    payloads: Vec<Bytes>,
    current_tick: Tick,
    current_real: Duration,
}

impl PacketLoopFixture {
    pub fn new(message_count: usize, payload_len: usize) -> Self {
        assert!(message_count > 0);
        let payloads = (0..message_count)
            .map(|message_index| {
                let value = (message_index % (u8::MAX as usize + 1)) as u8;
                Bytes::from(vec![value; payload_len])
            })
            .collect();
        Self {
            packet_builder: PacketBuilder::new(1.5),
            channel_id: 0,
            payloads,
            current_tick: Tick(0),
            current_real: Duration::default(),
        }
    }

    pub fn expected_messages(&self) -> usize {
        self.payloads.len()
    }

    pub fn expected_payload_bytes(&self) -> usize {
        self.payloads.iter().map(Bytes::len).sum()
    }

    pub fn expected_packets_for_messages(&self, total_messages: usize) -> usize {
        total_messages.div_ceil(self.expected_messages())
    }

    pub fn expected_payload_bytes_for_messages(&self, total_messages: usize) -> usize {
        let full_batches = total_messages / self.expected_messages();
        let remaining = total_messages % self.expected_messages();
        full_batches * self.expected_payload_bytes()
            + self
                .payloads
                .iter()
                .take(remaining)
                .map(Bytes::len)
                .sum::<usize>()
    }

    pub fn prepare_batch(&self) -> PacketLoopBatch {
        self.prepare_batch_with_message_count(self.expected_messages())
    }

    pub fn prepare_batches(&self, total_messages: usize) -> Vec<PacketLoopBatch> {
        let mut batches = Vec::with_capacity(self.expected_packets_for_messages(total_messages));
        let mut remaining = total_messages;
        while remaining > 0 {
            let batch_messages = remaining.min(self.expected_messages());
            batches.push(self.prepare_batch_with_message_count(batch_messages));
            remaining -= batch_messages;
        }
        batches
    }

    fn prepare_batch_with_message_count(&self, message_count: usize) -> PacketLoopBatch {
        let single_data = VecDeque::from_iter(
            self.payloads
                .iter()
                .take(message_count)
                .cloned()
                .map(|payload| SingleData::new(None, payload)),
        );
        PacketLoopBatch {
            single_data: vec![(self.channel_id, single_data)],
        }
    }

    pub fn run_batch(&mut self, batch: PacketLoopBatch) -> Result<PacketLoopStats, PacketError> {
        let real = self.current_real;
        self.packet_builder
            .header_manager
            .update(real, &LinkStats::default());
        self.packet_builder.header_manager.lost_packets.clear();

        let mut packets = self.packet_builder.build_packets_with_compression(
            real,
            self.current_tick,
            batch.single_data,
            Vec::new(),
            CompressionConfig::DISABLED,
        )?;
        self.current_tick += 1;
        self.current_real += Duration::from_millis(16);

        let mut stats = PacketLoopStats {
            packets: packets.len(),
            ..Default::default()
        };
        for packet in packets.drain(..) {
            self.parse_packet_payload(packet.payload.as_slice(), real, &mut stats)?;
            self.packet_builder.recycle_packet(packet);
        }
        self.packet_builder.recycle_packet_list(packets);
        Ok(stats)
    }

    fn parse_packet_payload(
        &mut self,
        payload: &[u8],
        real: Duration,
        stats: &mut PacketLoopStats,
    ) -> Result<(), PacketError> {
        let header = PacketHeader::read_from_prefix(payload)?;
        assert_eq!(header.get_packet_type(), PacketType::Data);
        let _ = self
            .packet_builder
            .header_manager
            .process_recv_packet_header(&header, real);
        self.packet_builder
            .header_manager
            .newly_acked_packets
            .clear();

        let mut reader = SlicePacketReader::new(
            payload
                .get(PacketHeader::BYTES..)
                .ok_or(SerializationError::InvalidValue)?,
        );
        while reader.has_remaining() {
            let channel_id = reader.read_varint()? as ChannelId;
            assert_eq!(channel_id, self.channel_id);
            let num_messages = reader.read_u8()?;
            for _ in 0..num_messages {
                match reader.read_u8()? {
                    0 => {}
                    1 => reader.skip(4)?,
                    _ => return Err(SerializationError::InvalidValue.into()),
                }
                let payload_len = reader.read_varint()? as usize;
                reader.skip(payload_len)?;
                stats.messages += 1;
                stats.payload_bytes += payload_len;
            }
        }
        Ok(())
    }

    pub fn run_batches(
        &mut self,
        batches: Vec<PacketLoopBatch>,
    ) -> Result<PacketLoopStats, PacketError> {
        let mut total = PacketLoopStats::default();
        for batch in batches {
            let stats = self.run_batch(batch)?;
            total.packets += stats.packets;
            total.messages += stats.messages;
            total.payload_bytes += stats.payload_bytes;
        }
        Ok(total)
    }
}
