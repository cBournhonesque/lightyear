use alloc::{vec, vec::Vec};
use bevy_platform::hash::FixedHasher;
#[cfg(feature = "test_utils")]
use core::convert::TryInto;
use core::time::Duration;
use indexmap::IndexMap;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::packet::packet::PacketId;
use crate::packet::packet_type::PacketType;
use crate::packet::stats_manager::packet::PacketStatsManager;
use lightyear_core::tick::Tick;
use lightyear_link::LinkStats;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};

/// Header included at the start of all packets
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PacketHeader {
    // TODO: this seems useless besides Data vs DataFragment
    /// Type of the packet sent
    packet_type: PacketType,
    /// Packet id from the sender's perspective
    pub(crate) packet_id: PacketId,
    /// Last ack-ed packet id received by the sender
    last_ack_packet_id: PacketId,
    /// Bitfield of the last 32 packet ids before `ack_id`
    /// (this means that in total we send acks for 33 packet-ids)
    /// See more information at: [GafferOnGames](https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/)
    ack_bitfield: u32,
    /// Current tick
    pub(crate) tick: Tick,
}

impl ToBytes for PacketHeader {
    fn bytes_len(&self) -> usize {
        1 + self.packet_id.bytes_len()
            + self.last_ack_packet_id.bytes_len()
            + 4
            + self.tick.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u8(self.packet_type as u8)?;
        buffer.write_u32(self.packet_id.0)?;
        buffer.write_u32(self.last_ack_packet_id.0)?;
        buffer.write_u32(self.ack_bitfield)?;
        buffer.write_u32(self.tick.0)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let packet_type = buffer.read_u8()?;
        let packet_id = buffer.read_u32()?;
        let last_ack_packet_id = buffer.read_u32()?;
        let ack_bitfield = buffer.read_u32()?;
        let tick = buffer.read_u32()?;
        Ok(Self {
            packet_type: PacketType::try_from(packet_type)?,
            packet_id: PacketId(packet_id),
            last_ack_packet_id: PacketId(last_ack_packet_id),
            ack_bitfield,
            tick: Tick(tick),
        })
    }
}

impl PacketHeader {
    /// Number of bytes written by [`PacketHeader::to_bytes`].
    pub(crate) const BYTES: usize = 17;
    /// Offset of the packet type byte inside the serialized header.
    pub(crate) const PACKET_TYPE_OFFSET: usize = 0;

    /// Parses a packet header from the start of a borrowed packet payload without taking ownership
    /// of the payload or constructing a [`Reader`].
    ///
    /// This is used by allocation-sensitive tests that parse a sent packet from `&[u8]` and then
    /// return the original `Vec<u8>` to `PacketBuilder`'s buffer pool. Normal receive code should
    /// keep using [`PacketHeader::from_bytes`], which advances a `Reader` past the header.
    #[cfg(feature = "test_utils")]
    #[doc(hidden)]
    pub(crate) fn read_from_prefix(bytes: &[u8]) -> Result<Self, SerializationError> {
        if bytes.len() < Self::BYTES {
            return Err(SerializationError::InvalidValue);
        }
        Ok(Self {
            packet_type: PacketType::try_from(bytes[Self::PACKET_TYPE_OFFSET])?,
            packet_id: PacketId(u32::from_be_bytes(bytes[1..5].try_into().unwrap())),
            last_ack_packet_id: PacketId(u32::from_be_bytes(bytes[5..9].try_into().unwrap())),
            ack_bitfield: u32::from_be_bytes(bytes[9..13].try_into().unwrap()),
            tick: Tick(u32::from_be_bytes(bytes[13..17].try_into().unwrap())),
        })
    }

    /// Get the value of the i-th bit in the bitfield (starting from the right-most bit, which is
    /// one PacketId below `last_ack_packet_id`
    ///
    /// i is 0-indexed. So 0 represents the first bit of the bitfield (starting from the right)
    fn get_bitfield_bit(&self, i: u8) -> bool {
        debug_assert!(i < ACK_BITFIELD_SIZE);
        self.ack_bitfield & (1 << i) != 0
    }

    pub fn get_packet_type(&self) -> PacketType {
        self.packet_type
    }
}

// we can only send acks for the last 32 packets ids before the last received packet
const ACK_BITFIELD_SIZE: u8 = 32;

/// minimum number of milliseconds after which we can consider a packet lost
/// (to avoid edge case behaviour)
const MIN_NACK_MILLIS: u64 = 10;

/// maximum number of seconds after which we consider a packet lost
const MAX_NACK_SECONDS: u64 = 3;

/// Keeps track of sent and received packets to be able to write the packet headers correctly
/// For more information: [GafferOnGames](https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/)
#[derive(Debug)]
pub struct PacketHeaderManager {
    // Local packet id which we'll bump each time we send a new packet over the network.
    // (we always increment the packet_id, even when we resend a lost packet)
    next_packet_id: PacketId,
    /// ACK-eliciting packets whose delivery has not yet been classified.
    ///
    /// ACK-only packets are deliberately omitted. Their packet type tells the peer not to schedule
    /// a response solely to acknowledge them, which prevents ACK ping-pong. A later data packet may
    /// acknowledge them incidentally, but no response is guaranteed; tracking them would therefore
    /// misclassify an idle peer's ACK-only packets as lost.
    sent_packets_not_acked: IndexMap<PacketId, Duration, FixedHasher>,
    stats_manager: PacketStatsManager,
    pub(crate) lost_packets: Vec<PacketId>,
    pub(crate) newly_acked_packets: IndexMap<PacketId, Duration, FixedHasher>,

    // keep track of the packets that were received (last packet received and the
    // `ACK_BITFIELD_SIZE` packets before that)
    recv_buffer: ReceiveBuffer,
    /// Whether an acknowledgement-eliciting packet has arrived since the last packet we sent.
    ack_pending: bool,
    /// After how many multiples of RTT do we consider a packet to be lost?
    ///
    /// The default is 1.5; i.e. after 1.5 times the round trip time, we consider a packet lost if
    /// we haven't received an ACK for it.
    nack_rtt_multiple: f32,
}

impl Default for PacketHeaderManager {
    fn default() -> Self {
        Self::new(1.5)
    }
}

impl PacketHeaderManager {
    pub(crate) fn new(nack_rtt_multiple: f32) -> Self {
        Self {
            next_packet_id: PacketId(0),
            stats_manager: PacketStatsManager::default(),
            sent_packets_not_acked: IndexMap::default(),
            lost_packets: vec![],
            newly_acked_packets: IndexMap::default(),
            recv_buffer: ReceiveBuffer::new(),
            ack_pending: false,
            nack_rtt_multiple,
        }
    }

    /// Internal bookkeeping. Updates the list of packets that are NACKed (acknowledged as losts)
    pub(crate) fn update(&mut self, real: Duration, link_stats: &LinkStats) {
        self.stats_manager.update(real);
        let rtt = link_stats.rtt;
        let nack_duration = rtt
            .mul_f32(self.nack_rtt_multiple)
            .min(Duration::from_secs(MAX_NACK_SECONDS))
            .max(Duration::from_millis(MIN_NACK_MILLIS));
        // clear sent packets that haven't received any ack for a while
        self.sent_packets_not_acked.retain(|packet_id, time_sent| {
            // protection against keep old packets for too long (which would cause bugs on wraparound)
            if real.saturating_sub(*time_sent) > nack_duration
                || (self.next_packet_id - *packet_id > i32::MAX / 3)
            {
                trace!(?packet_id, "sent packet got lost");
                self.lost_packets.push(*packet_id);
                self.stats_manager.sent_packet_lost();
                return false;
            }
            true
        });
    }

    /// Process the header of a received packet (update ack metadata)
    ///
    /// Returns the list of packets that have been newly acked by the remote
    pub(crate) fn process_recv_packet_header(
        &mut self,
        header: &PacketHeader,
        real: Duration,
    ) -> Vec<(PacketId, Duration)> {
        let mut newly_acked_packets = vec![];
        // update the receive buffer
        self.stats_manager.received_packet();
        self.recv_buffer.recv_packet(header.packet_id);
        self.ack_pending |= header.packet_type.is_ack_eliciting();

        // read the ack information (ack id + ack bitfield) from the received header, and update
        // the list of our sent packets that have not been acked yet
        if let Some((packet, time_sent)) =
            self.update_sent_packets_not_acked(&header.last_ack_packet_id)
        {
            self.record_newly_acked_packet(packet, real, time_sent, &mut newly_acked_packets);
        }
        for i in 1..=ACK_BITFIELD_SIZE {
            let packet_id = PacketId(header.last_ack_packet_id.wrapping_sub(i as u32));
            if header.get_bitfield_bit(i - 1)
                && let Some((packet, time_sent)) = self.update_sent_packets_not_acked(&packet_id)
            {
                self.record_newly_acked_packet(packet, real, time_sent, &mut newly_acked_packets);
            }
        }
        newly_acked_packets
    }

    fn record_newly_acked_packet(
        &mut self,
        packet: PacketId,
        real: Duration,
        time_sent: Duration,
        newly_acked_packets: &mut Vec<(PacketId, Duration)>,
    ) {
        self.stats_manager.sent_packet_acked();
        let rtt_sample = real.saturating_sub(time_sent);
        self.newly_acked_packets.insert(packet, rtt_sample);
        newly_acked_packets.push((packet, rtt_sample));
    }

    /// Update the list of sent packets that have not been acked yet
    /// when we receive confirmation that packet_id was delivered
    ///
    /// Also potentially notify the channels/etc. that the packet was delivered.
    fn update_sent_packets_not_acked(
        &mut self,
        packet_id: &PacketId,
    ) -> Option<(PacketId, Duration)> {
        if self.sent_packets_not_acked.contains_key(packet_id) {
            let time_sent = self.sent_packets_not_acked.swap_remove(packet_id)?;
            return Some((*packet_id, time_sent));
        }
        None
    }

    /// Preview the header of the next packet without changing sent-packet state.
    pub(crate) fn preview_send_packet_header(
        &self,
        packet_type: PacketType,
        tick: Tick,
    ) -> PacketHeader {
        // if we didn't have a last packet id, start with the maximum value
        // (so that receiving 0 counts as an update)
        let last_ack_packet_id = self
            .recv_buffer
            .last_recv_packet_id
            .unwrap_or(PacketId(u32::MAX));
        PacketHeader {
            packet_type,
            packet_id: self.next_packet_id,
            last_ack_packet_id,
            ack_bitfield: self.recv_buffer.get_bitfield(),
            tick,
        }
    }

    /// Commit a previewed packet after its final payload has entered `Link.send`.
    pub(crate) fn commit_send_packet(&mut self, packet_id: PacketId, real: Duration) {
        debug_assert_eq!(
            packet_id, self.next_packet_id,
            "packets must be committed in preview order"
        );
        self.stats_manager.sent_ack_eliciting_packet();
        trace!(?self.next_packet_id, "Sent packet");
        self.sent_packets_not_acked.insert(packet_id, real);
        self.next_packet_id += 1;
        self.ack_pending = false;
    }

    /// Commits a header-only acknowledgement without waiting for an acknowledgement of it.
    ///
    /// The peer does not schedule a packet merely to ACK this one; that non-ACK-eliciting behavior
    /// is what prevents ACK ping-pong. Consequently, the packet is not inserted into
    /// `sent_packets_not_acked` and is excluded from the packet-loss denominator: without a
    /// guaranteed response, silence cannot distinguish successful delivery from packet loss.
    pub(crate) fn commit_send_ack_only(&mut self, packet_id: PacketId) {
        debug_assert_eq!(
            packet_id, self.next_packet_id,
            "packets must be committed in preview order"
        );
        self.stats_manager.sent_ack_only_packet();
        trace!(?self.next_packet_id, "Sent ACK-only packet");
        self.next_packet_id += 1;
        self.ack_pending = false;
    }

    pub(crate) fn has_pending_ack(&self) -> bool {
        self.ack_pending
    }
}

/// Data structure to keep track of the ids of the received packets
#[derive(Debug)]
pub struct ReceiveBuffer {
    /// The packet id of the most recent packet received
    last_recv_packet_id: Option<PacketId>,
    /// Use a ring buffer of ACK_BITFIELD_SIZE to track if we received the last
    /// ACK_BITFIELD_SIZE packets prior to the last received packet
    buffer: ConstGenericRingBuffer<bool, { ACK_BITFIELD_SIZE as usize }>,
}

impl Default for ReceiveBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl ReceiveBuffer {
    fn new() -> Self {
        let mut buffer = ConstGenericRingBuffer::new();
        // start with false (we haven't received any packet yet)
        buffer.fill(false);
        Self {
            last_recv_packet_id: None,
            buffer,
        }
    }

    /// Receive a new packet id and update the receive buffer accordingly
    fn recv_packet(&mut self, id: PacketId) {
        // special case: this is the first packet we receive
        if self.last_recv_packet_id.is_none() {
            self.last_recv_packet_id = Some(id);
            return;
        }

        let bitfield_size = ACK_BITFIELD_SIZE as i32;
        let diff = self.last_recv_packet_id.unwrap() - id;
        if diff > bitfield_size {
            return;
        }
        // the packet id is in the existing bitfield; update the corresponding bit
        if diff > 0 {
            let recv_bit = self
                .buffer
                .get_mut_signed(-diff as isize)
                .expect("ring buffer should be full");
            *recv_bit = true;
        }
        // the packet id is the most recent
        if diff < 0 {
            // update the bitfield
            // optimization: if the new message is very far ahead, we can reset the bitfield
            if diff < -(bitfield_size + 1) {
                self.buffer.fill(false);
            } else {
                self.buffer.enqueue(true);
                // add False for all the packets in between the old and new last_recv_packet_id
                for _ in 0..(diff.abs() - 1) {
                    self.buffer.enqueue(false);
                }
            }

            // update the most recent packet received
            self.last_recv_packet_id = Some(id);
        }
    }

    /// Convert the Receive Buffer to the bitfield that we need to send in the PacketHeader
    fn get_bitfield(&self) -> u32 {
        let mut ack_bitfield: u32 = 0;
        // mask starting from the left
        let mut mask = 1 << (ACK_BITFIELD_SIZE - 1);

        // iter goes from the item pushed the longest ago (to the left of the bitfield)
        // to the items pushed most recently (to the right of the bitfield)
        for exists in self.buffer.iter() {
            if *exists {
                ack_bitfield |= mask;
            }
            mask >>= 1;
        }
        ack_bitfield
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use core::time::Duration;
    use lightyear_serde::ToBytes;

    use super::*;

    fn prepare_send_packet_header(
        manager: &mut PacketHeaderManager,
        packet_type: PacketType,
        real: Duration,
        tick: Tick,
    ) -> PacketHeader {
        let header = manager.preview_send_packet_header(packet_type, tick);
        manager.commit_send_packet(header.packet_id, real);
        header
    }

    #[test]
    fn test_recv_buffer() {
        let recv_buffer = ReceiveBuffer::new();
        assert_eq!(recv_buffer.last_recv_packet_id, None);
        assert_eq!(recv_buffer.get_bitfield(), 0);

        // add a most recent packet, and perform some assertions
        fn add_most_recent_packet(
            mut buffer: ReceiveBuffer,
            id: u32,
            expected_bitfield: u32,
        ) -> ReceiveBuffer {
            buffer.recv_packet(PacketId(id));
            assert_eq!(buffer.last_recv_packet_id, Some(PacketId(id)));
            assert_eq!(buffer.get_bitfield(), expected_bitfield);
            buffer
        }

        // receive one packet with increment 1
        let recv_buffer = add_most_recent_packet(recv_buffer, 0, 0);

        // receive one more packet with increment 1
        let recv_buffer = add_most_recent_packet(recv_buffer, 1, 1);

        // receive a packet where the ACK_BITFIELD_SIZE > diff_id > 0
        let recv_buffer = add_most_recent_packet(recv_buffer, 3, 0b0000_0110u32);

        // receive another packet where the ACK_BITFIELD_SIZE > diff_id > 0
        let mut recv_buffer = add_most_recent_packet(recv_buffer, 6, 0b0011_0100u32);

        // receive a packet which is in the past
        // -ACK_BITFIELD_SIZE < diff_id < 0
        recv_buffer.recv_packet(PacketId(2));
        assert_eq!(recv_buffer.last_recv_packet_id, Some(PacketId(6)));
        assert_eq!(recv_buffer.get_bitfield(), 0b0011_1100u32);

        // receive a packet that is far ahead
        // diff > ACK_BITFIELD_SIZE
        let recv_buffer = add_most_recent_packet(recv_buffer, 50, 0);

        // receive a packet at the max far ahead
        // diff == ACK_BITFIELD_SIZE
        let mut recv_buffer = add_most_recent_packet(recv_buffer, 82, 1 << (32 - 1));

        // receive a packet that is too far in the past
        // diff_id < -ACK_BITFIELD_SIZE
        recv_buffer.recv_packet(PacketId(49));
        assert_eq!(recv_buffer.last_recv_packet_id, Some(PacketId(82)));
        assert_eq!(recv_buffer.get_bitfield(), 1 << (32 - 1));
    }

    #[test]
    fn test_serde_header() -> Result<(), SerializationError> {
        let header = PacketHeader {
            packet_type: PacketType::Data,
            packet_id: PacketId(27),
            last_ack_packet_id: PacketId(13),
            ack_bitfield: 3,
            tick: Tick(6),
        };
        let mut writer = Vec::new();
        header.to_bytes(&mut writer)?;
        assert_eq!(writer.len(), header.bytes_len());

        let mut reader = writer.into();
        let read_header = PacketHeader::from_bytes(&mut reader)?;
        assert_eq!(header, read_header);
        Ok(())
    }

    #[test]
    fn preview_does_not_advance_packet_state_until_commit() {
        let mut manager = PacketHeaderManager::new(1.5);

        let first = manager.preview_send_packet_header(PacketType::Data, Tick(1));
        let repeated = manager.preview_send_packet_header(PacketType::Data, Tick(2));
        assert_eq!(first.packet_id, PacketId(0));
        assert_eq!(repeated.packet_id, PacketId(0));
        assert!(manager.sent_packets_not_acked.is_empty());

        manager.commit_send_packet(first.packet_id, Duration::from_millis(10));
        let next = manager.preview_send_packet_header(PacketType::Data, Tick(3));
        assert_eq!(next.packet_id, PacketId(1));
        assert_eq!(
            manager.sent_packets_not_acked.get(&PacketId(0)),
            Some(&Duration::from_millis(10))
        );
    }

    #[test]
    fn ack_only_commit_advances_sequence_without_entering_loss_tracking() {
        let mut manager = PacketHeaderManager::new(1.5);
        let ack_only = manager.preview_send_packet_header(PacketType::AckOnly, Tick(1));

        manager.commit_send_ack_only(ack_only.packet_id);

        assert!(manager.sent_packets_not_acked.is_empty());
        assert_eq!(
            manager
                .preview_send_packet_header(PacketType::Data, Tick(2))
                .packet_id,
            PacketId(1)
        );
        manager.update(Duration::from_secs(1), &LinkStats::default());
        assert!(manager.lost_packets.is_empty());
    }

    #[test]
    fn packet_ack_produces_rtt_sample_from_latest_ack() {
        let mut sender = PacketHeaderManager::new(1.5);
        let mut receiver = PacketHeaderManager::new(1.5);

        let sent_header = prepare_send_packet_header(
            &mut sender,
            PacketType::Data,
            Duration::from_millis(10),
            Tick(1),
        );
        assert!(
            receiver
                .process_recv_packet_header(&sent_header, Duration::from_millis(25))
                .is_empty()
        );

        let ack_header = prepare_send_packet_header(
            &mut receiver,
            PacketType::Data,
            Duration::from_millis(25),
            Tick(2),
        );
        let acked_packets =
            sender.process_recv_packet_header(&ack_header, Duration::from_millis(60));

        assert_eq!(
            acked_packets,
            vec![(PacketId(0), Duration::from_millis(50))]
        );
        assert_eq!(
            sender.newly_acked_packets.get(&PacketId(0)),
            Some(&Duration::from_millis(50))
        );
    }

    #[test]
    fn packet_ack_bitfield_produces_rtt_samples_for_older_packets() {
        let mut sender = PacketHeaderManager::new(1.5);
        let mut receiver = PacketHeaderManager::new(1.5);

        let sent_0 = prepare_send_packet_header(
            &mut sender,
            PacketType::Data,
            Duration::from_millis(10),
            Tick(1),
        );
        let _sent_1 = prepare_send_packet_header(
            &mut sender,
            PacketType::Data,
            Duration::from_millis(20),
            Tick(2),
        );
        let sent_2 = prepare_send_packet_header(
            &mut sender,
            PacketType::Data,
            Duration::from_millis(30),
            Tick(3),
        );

        receiver.process_recv_packet_header(&sent_0, Duration::from_millis(15));
        receiver.process_recv_packet_header(&sent_2, Duration::from_millis(35));

        let ack_header = prepare_send_packet_header(
            &mut receiver,
            PacketType::Data,
            Duration::from_millis(40),
            Tick(4),
        );
        let acked_packets =
            sender.process_recv_packet_header(&ack_header, Duration::from_millis(80));

        assert_eq!(
            acked_packets,
            vec![
                (PacketId(2), Duration::from_millis(50)),
                (PacketId(0), Duration::from_millis(70)),
            ]
        );
        assert!(!sender.newly_acked_packets.contains_key(&PacketId(1)));
    }
}
