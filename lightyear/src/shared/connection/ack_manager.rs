use std::collections::HashMap;

use crate::shared::{MessageManager, PacketIndex, sequence_greater_than};

use super::{
    packet_notifiable::PacketNotifiable, packet_type::PacketType, sequence_buffer::SequenceBuffer,
    standard_header::StandardHeader,
};

pub const REDUNDANT_PACKET_ACKS_SIZE: u16 = 32;
const DEFAULT_SEND_PACKETS_SIZE: usize = 256;

/// Keeps track of sent & received packets, and contains ack information that is
/// copied into the standard header on each outgoing packet
pub struct AckManager {
    // Local packet index which we'll bump each time we send a new packet over the network.
    next_packet_index: PacketIndex,
    // The last acked packet index of the packets we've sent to the remote host.
    last_recv_packet_index: PacketIndex,
    // Using a `Hashmap` to track every packet we send out so we can ensure that we can resend when
    // dropped.
    sent_packets: HashMap<PacketIndex, SentPacket>,
    // However, we can only reasonably ack up to `REDUNDANT_PACKET_ACKS_SIZE + 1` packets on each
    // message we send so this should be that large.
    received_packets: SequenceBuffer<ReceivedPacket>,
}

impl AckManager {
    pub fn new() -> Self {
        Self {
            next_packet_index: 0,
            last_recv_packet_index: u16::MAX,
            sent_packets: HashMap::with_capacity(DEFAULT_SEND_PACKETS_SIZE),
            received_packets: SequenceBuffer::with_capacity(REDUNDANT_PACKET_ACKS_SIZE + 1),
        }
    }

    /// Get the index of the next outgoing packet
    pub fn next_sender_packet_index(&self) -> PacketIndex {
        self.next_packet_index
    }

    /// Process an incoming packet, handle notifications of delivered / dropped
    /// packets
    pub fn process_incoming_header(
        &mut self,
        header: &StandardHeader,
        message_manager: &mut MessageManager,
        packet_notifiable: &mut Option<&mut dyn PacketNotifiable>,
    ) {
        let sender_packet_index = header.sender_packet_index;
        let sender_ack_index = header.sender_ack_index;
        let mut sender_ack_bitfield = header.sender_ack_bitfield;

        self.received_packets
            .insert(sender_packet_index, ReceivedPacket {});

        // ensure that `self.sender_ack_index` is always increasing (with
        // wrapping)
        if sequence_greater_than(sender_ack_index, self.last_recv_packet_index) {
            self.last_recv_packet_index = sender_ack_index;
        }

        // the current `sender_ack_index` was (clearly) received so we should remove it
        if let Some(sent_packet) = self.sent_packets.get(&sender_ack_index) {
            if sent_packet.packet_type == PacketType::Data {
                self.notify_packet_delivered(sender_ack_index, message_manager, packet_notifiable);
            }

            self.sent_packets.remove(&sender_ack_index);
        }

        // The `sender_ack_bitfield` is going to include whether or not the past 32
        // packets have been received successfully.
        // If so, we have no need to resend old packets.
        for i in 1..=REDUNDANT_PACKET_ACKS_SIZE {
            let sent_packet_index = sender_ack_index.wrapping_sub(i);
            if let Some(sent_packet) = self.sent_packets.get(&sent_packet_index) {
                if sender_ack_bitfield & 1 == 1 {
                    if sent_packet.packet_type == PacketType::Data {
                        self.notify_packet_delivered(
                            sent_packet_index,
                            message_manager,
                            packet_notifiable,
                        );
                    }

                    self.sent_packets.remove(&sent_packet_index);
                } else {
                    self.sent_packets.remove(&sent_packet_index);
                }
            }

            sender_ack_bitfield >>= 1;
        }
    }

    /// Records the packet with the given packet index
    fn track_packet(&mut self, packet_type: PacketType, packet_index: PacketIndex) {
        self.sent_packets
            .insert(packet_index, SentPacket { packet_type });
    }

    /// Bumps the local packet index
    fn increment_local_packet_index(&mut self) {
        self.next_packet_index = self.next_packet_index.wrapping_add(1);
    }

    pub fn next_outgoing_packet_header(&mut self, packet_type: PacketType) -> StandardHeader {
        let next_packet_index = self.next_sender_packet_index();

        let outgoing = StandardHeader::new(
            packet_type,
            next_packet_index,
            self.last_received_packet_index(),
            self.ack_bitfield(),
        );

        self.track_packet(packet_type, next_packet_index);
        self.increment_local_packet_index();

        outgoing
    }

    fn notify_packet_delivered(
        &self,
        sent_packet_index: PacketIndex,
        message_manager: &mut MessageManager,
        packet_notifiable: &mut Option<&mut dyn PacketNotifiable>,
    ) {
        message_manager.notify_packet_delivered(sent_packet_index);
        if let Some(notifiable) = packet_notifiable {
            notifiable.notify_packet_delivered(sent_packet_index);
        }
    }

    fn last_received_packet_index(&self) -> PacketIndex {
        self.received_packets.sequence_num().wrapping_sub(1)
    }

    fn ack_bitfield(&self) -> u32 {
        let last_received_remote_packet_index: PacketIndex = self.last_received_packet_index();
        let mut ack_bitfield: u32 = 0;
        let mut mask: u32 = 1;

        // iterate the past `REDUNDANT_PACKET_ACKS_SIZE` received packets and set the
        // corresponding bit for each packet which exists in the buffer.
        for i in 1..=REDUNDANT_PACKET_ACKS_SIZE {
            let received_packet_index = last_received_remote_packet_index.wrapping_sub(i);
            if self.received_packets.exists(received_packet_index) {
                ack_bitfield |= mask;
            }
            mask <<= 1;
        }

        ack_bitfield
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SentPacket {
    pub packet_type: PacketType,
}

#[derive(Clone, Debug, Default)]
pub struct ReceivedPacket;
