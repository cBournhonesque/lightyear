use crate::channel::channel::ChannelHeader;
use crate::packet::packet_type::PacketType;
use crate::packet::wrapping_id::PacketId;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::collections::HashMap;

/// Header included at the start of all packets
// TODO: use packet_struct for encoding
#[derive(bitcode::Decode, bitcode::Encode, Debug, Clone, Copy)]
pub(crate) struct PacketHeader {
    /// General id for the protocol used
    // TODO: add CRC check (see https://gafferongames.com/post/serialization_strategies/)
    protocol_id: u16,
    /// Type of the packet sent
    packet_type: PacketType,
    /// Packet id from the sender's perspective
    packet_id: PacketId,
    /// Last ack-ed packet id received by the sender
    last_ack_packet_id: PacketId,
    /// Bitfield of the last 32 packet ids before `ack_id`
    /// (this means that in total we send acks for 33 packet-ids)
    /// See more information at: https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/
    ack_bitfield: u32,

    pub(crate) channel_header: ChannelHeader,
    // /// Extra data to be included in the header (channel id, maybe fragmented id, tick?)
    // extra_header: Box<dyn PacketHeaderData>,
}

pub trait PacketHeaderData {
    fn encode(&self) -> Vec<u8>;
    fn decode(&mut self, data: &[u8]);
}

impl PacketHeader {
    /// Get the value of the i-th bit in the bitfield (starting from the right-most bit, which is
    /// one PacketId below `last_ack_packet_id`
    ///
    /// i is 0-indexed. So 0 represents the first bit of the bitfield (starting from the right)
    fn get_bitfield_bit(&self, i: u8) -> bool {
        assert!(i < ACK_BITFIELD_SIZE);
        self.ack_bitfield & (1 << i) != 0
    }
}

// we can only send acks for the last 32 packets ids before the last received packet
const ACK_BITFIELD_SIZE: u8 = 32;
// we can only buffer up to `MAX_SEND_PACKET_QUEUE_SIZE` packets for sending
const MAX_SEND_PACKET_QUEUE_SIZE: u8 = 255;

/// Keeps track of sent and received packets to be able to write the packet headers correctly
/// For more information: https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/
pub struct PacketHeaderManager {
    // Local packet id which we'll bump each time we send a new packet over the network.
    // (we always increment the packet_id, even when we resend a lost packet)
    next_packet_id: PacketId,
    // keep track of the packets we send out and that have not been acked yet,
    // so we can resend them when dropped
    sent_packets_not_acked: HashMap<PacketId, ()>,
    // keep track of the packets that were received (last packet received and the
    // `ACK_BITFIELD_SIZE` packets before that)
    recv_buffer: ReceiveBuffer,
}

impl PacketHeaderManager {
    pub fn new() -> Self {
        Self {
            next_packet_id: PacketId(0),
            sent_packets_not_acked: HashMap::with_capacity(MAX_SEND_PACKET_QUEUE_SIZE as usize),
            recv_buffer: ReceiveBuffer::new(),
        }
    }

    /// Return the packet id of the next packet to be sent
    pub fn next_packet_id(&self) -> PacketId {
        self.next_packet_id
    }

    /// Increment the packet id of the next packet to be sent
    pub fn increment_next_packet_id(&mut self) {
        self.next_packet_id = PacketId(self.next_packet_id.wrapping_add(1));
    }

    /// Process the header of a received packet (update ack metadata)
    pub fn process_recv_packet_header(&mut self, header: &PacketHeader) {
        // update the receive buffer
        self.recv_buffer.recv_packet(header.packet_id);

        // read the ack information (ack id + ack bitfield) from the received header, and update
        // the list of our sent packets that have not been acked yet
        self.sent_packets_not_acked
            .remove(&header.last_ack_packet_id);
        for i in 1..=ACK_BITFIELD_SIZE {
            let packet_id = PacketId(header.last_ack_packet_id.wrapping_sub(i as u16));
            if header.get_bitfield_bit(i - 1) {
                self.sent_packets_not_acked.remove(&packet_id);
            }
        }
    }

    /// Prepare the header of the next packet to send
    pub fn prepare_send_packet_header(
        &mut self,
        protocol_id: u16,
        packet_type: PacketType,
        channel_header: ChannelHeader,
    ) -> PacketHeader {
        // if we didn't have a last packet id, start with the maximum value
        // (so that receiving 0 counts as an update)
        let last_ack_packet_id = match self.recv_buffer.last_recv_packet_id {
            Some(id) => id,
            None => PacketId(u16::MAX),
        };
        let outgoing_header = PacketHeader {
            protocol_id,
            packet_type,
            packet_id: self.next_packet_id,
            last_ack_packet_id,
            ack_bitfield: self.recv_buffer.get_bitfield(),
            channel_header,
            // extra_header: Box::new(()),
        };
        self.increment_next_packet_id();
        outgoing_header
    }
}

/// Data structure to keep track of the ids of the received packets
pub struct ReceiveBuffer {
    /// The packet id of the most recent packet received
    last_recv_packet_id: Option<PacketId>,
    /// Use a ring buffer of ACK_BITFIELD_SIZE to track if we received the last
    /// ACK_BITFIELD_SIZE packets prior to the last received packet
    buffer: ConstGenericRingBuffer<bool, { ACK_BITFIELD_SIZE as usize }>,
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

        let bitfield_size = ACK_BITFIELD_SIZE as i16;
        let diff = self.last_recv_packet_id.unwrap() - id;
        if diff > bitfield_size {
            return;
        }
        // the packet id is in the existing bitfield; update the corresponding bit
        if diff > 0 {
            let mut recv_bit = self
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
                self.buffer.push(true);
                // add False for all the packets in between the old and new last_recv_packet_id
                for _ in 0..(diff.abs() - 1) {
                    self.buffer.push(false);
                }
            }

            // update the most recent packet received
            self.last_recv_packet_id = Some(id);
        }

        ()
    }

    /// Convert the Receive Buffer to the bitfield that we need to send in the PacketHeader
    fn get_bitfield(&self) -> u32 {
        let mut ack_bitfield: u32 = 0;
        // mask starting from the left
        let mut mask = 1 << (ACK_BITFIELD_SIZE - 1);

        // iter goes from the item pushed the longest ago (to the left of the bitfield)
        // to the items pushed most recently (to the right of the bitfield)
        for (i, &exists) in self.buffer.iter().enumerate() {
            if exists {
                ack_bitfield |= mask;
            }
            mask >>= 1;
        }
        ack_bitfield
    }
}

#[cfg(test)]
mod tests {
    use super::PacketId;
    use super::ReceiveBuffer;

    #[test]
    fn test_recv_buffer() {
        let mut recv_buffer = ReceiveBuffer::new();
        assert_eq!(recv_buffer.last_recv_packet_id, None);
        assert_eq!(recv_buffer.get_bitfield(), 0);

        // add a most recent packet, and perform some assertions
        fn add_most_recent_packet(
            mut buffer: ReceiveBuffer,
            id: u16,
            expected_bitfield: u32,
        ) -> ReceiveBuffer {
            buffer.recv_packet(PacketId(id));
            assert_eq!(buffer.last_recv_packet_id, Some(PacketId(id)));
            assert_eq!(buffer.get_bitfield(), expected_bitfield);
            buffer
        };

        // receive one packet with increment 1
        let recv_buffer = add_most_recent_packet(recv_buffer, 0, 0);

        // receive one more packet with increment 1
        let recv_buffer = add_most_recent_packet(recv_buffer, 1, 1);

        // receive a packet where the ACK_BITFIELD_SIZE > diff_id > 0
        let recv_buffer = add_most_recent_packet(recv_buffer, 3, 0b0000_0110 as u32);

        // receive another packet where the ACK_BITFIELD_SIZE > diff_id > 0
        let mut recv_buffer = add_most_recent_packet(recv_buffer, 6, 0b0011_0100 as u32);

        // receive a packet which is in the past
        // -ACK_BITFIELD_SIZE < diff_id < 0
        recv_buffer.recv_packet(PacketId(2));
        assert_eq!(recv_buffer.last_recv_packet_id, Some(PacketId(6)));
        assert_eq!(recv_buffer.get_bitfield(), 0b0011_1100 as u32);

        // receive a packet that is far ahead
        // diff > ACK_BITFIELD_SIZE
        let recv_buffer = add_most_recent_packet(recv_buffer, 50, 0);

        // receive a packet at the max far ahead
        // diff == ACK_BITFIELD_SIZE
        let mut recv_buffer = add_most_recent_packet(recv_buffer, 82, 1 << 32 - 1);

        // receive a packet that is too far in the past
        // diff_id < -ACK_BITFIELD_SIZE
        recv_buffer.recv_packet(PacketId(49));
        assert_eq!(recv_buffer.last_recv_packet_id, Some(PacketId(82)));
        assert_eq!(recv_buffer.get_bitfield(), 1 << 32 - 1);
    }
}
