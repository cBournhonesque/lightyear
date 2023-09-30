use crate::packet::header::PacketHeaderManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::{Packet, SinglePacket, MTU_PACKET_BYTES};
use crate::packet::packet_type::PacketType;
use crate::registry::channel::{ChannelKind, ChannelRegistry};
use crate::registry::message::MessageRegistry;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use anyhow::anyhow;
use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::write::Write;
use bitcode::Buffer;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = 1 * MTU_PACKET_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    pub(crate) header_manager: PacketHeaderManager,
    channel_registry: &'static ChannelRegistry,
    message_registry: &'static MessageRegistry,
    num_bits_available: usize,

    /// Pre-allocated buffer to encode/decode without allocation.
    try_write_buffer: WriteWordBuffer,
    write_buffer: WriteWordBuffer,
}

// PLAN:
// -

impl PacketManager {
    pub fn new(
        channel_registry: &'static ChannelRegistry,
        message_registry: &'static MessageRegistry,
    ) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            channel_registry,
            message_registry,
            num_bits_available: MTU_PACKET_BYTES * 8,
            /// write buffer to encode packets bit by bit
            // TODO: create a BufWriter to keep track of both the buffer and the Writer. 
            try_write_buffer: WriteBuffer::with_capacity(10 * PACKET_BUFFER_CAPACITY),
            write_buffer: WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    /// Reset the buffers used to encode packets
    pub fn clear_write_buffers(&mut self) {
        self.try_write_buffer = WriteBuffer::with_capacity(10 * PACKET_BUFFER_CAPACITY);
        self.write_buffer = WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY);
    }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet) -> anyhow::Result<&[u8]> {
        // TODO: check that we haven't allocated!

        // Create a write buffer with capacity the size of a packet
        // let mut write_buffer = Buffer::with_capacity(MTU_PACKET_BYTES);
        // let mut writer = write_buffer.0.start_write();
        packet.encode(self.message_registry, &mut self.write_buffer)?;
        let bytes = self.write_buffer.finish_write();
        // let bytes = write_buffer.0.finish_write(writer);
        Ok(bytes)
    }

    /// Decode a packet from raw bytes
    // TODO: the reader buffer will be created from the io (we copy the io bytes into a buffer)
    pub(crate) fn decode_packet(&mut self, reader: &mut impl ReadBuffer) -> anyhow::Result<Packet> {
        Packet::decode(self.message_registry, reader)
    }

    /// Start building new packet
    pub(crate) fn build_new_packet(&mut self) -> Packet {
        self.clear_write_buffers();

        Packet::Single(SinglePacket {
            // TODO: handle protocol and packet type
            header: self.header_manager.prepare_send_packet_header(
                0,
                PacketType::Data,
                // ChannelHeader {
                //     kind: ChannelKind(0),
                // },
            ),
            data: vec![],
        })
    }

    /// Returns true if there's enough space in the current packet to add a message
    /// The expectation is that we only work on a single packet at a time.
    pub fn can_add_message(
        &mut self,
        packet: &mut Packet,
        message: &MessageContainer,
    ) -> anyhow::Result<bool> {
        match packet {
            Packet::Single(single_packet) => {
                // TODO: either
                //  - get a function on the encoder that computes the amount of bits that the serialization will take
                //  - or we serialize and check the amount of bits it took

                // try to serialize in the try buffer
                let num_bits = message.encode(self.message_registry, &mut self.try_write_buffer)?;

                if num_bits <= self.num_bits_available {
                    self.num_bits_available -= num_bits;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            _ => unimplemented!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::packet::manager::PacketManager;
    use crate::packet::message::MessageContainer;
    use crate::packet::packet::MTU_PACKET_BYTES;

    use bytes::Bytes;
    //
    // #[test]
    // fn test_write_small_message() -> anyhow::Result<()> {
    //     let mut manager = PacketManager::new();
    //
    //     let small_message = MessageContainer::new(Bytes::from("small"));
    //     let mut packet = manager.build_new_packet();
    //     manager.try_add_message(&mut packet, small_message.clone())?;
    //
    //     assert_eq!(packet.num_messages(), 1);
    //
    //     manager.try_add_message(&mut packet, small_message.clone())?;
    //     assert_eq!(packet.num_messages(), 2);
    //     Ok(())
    // }
    //
    // #[test]
    // fn test_write_big_message() -> anyhow::Result<()> {
    //     let mut manager = PacketManager::new();
    //
    //     let big_bytes = vec![1u8; 2 * MTU_PACKET_BYTES];
    //     let big_message = MessageContainer::new(Bytes::from(big_bytes));
    //     let mut packet = manager.build_new_packet();
    //     let error = manager
    //         .try_add_message(&mut packet, big_message)
    //         .unwrap_err();
    //     let root_cause = error.root_cause();
    //     assert_eq!(
    //         format!("{}", root_cause),
    //         "Message too big to fit in packet"
    //     );
    //     Ok(())
    // }
}
