use crate::packet::header::PacketHeaderManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::{Packet, SinglePacket, MTU_PACKET_BYTES};
use crate::packet::packet_type::PacketType;
use crate::registry::channel::{ChannelKind, ChannelRegistry};
use crate::registry::message::MessageRegistry;
use anyhow::anyhow;
use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::Buffer;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = 10 * MTU_PACKET_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    pub(crate) header_manager: PacketHeaderManager,
    channel_registry: &'static ChannelRegistry,
    message_registry: &'static MessageRegistry,
    num_bytes_available: usize,
    /// Pre-allocated buffer to encode/decode without allocation
    bytes_buffer: Buffer,
}

// PLAN:
// -

impl PacketManager {
    pub fn new(channel_registry: &ChannelRegistry, message_registry: &MessageRegistry) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            channel_registry,
            message_registry,
            num_bytes_available: MTU_PACKET_BYTES,
            /// write buffer to encode packets bit by bit
            bytes_buffer: Buffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    // /// Returns true if the given number of bits can fit into the current packet
    // fn can_fit(&self, num_bytes: u32) -> bool {
    //     num_bytes <= self.num_bytes_available
    // }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet) -> anyhow::Result<&[u8]> {
        // Create a write buffer with capacity the size of a packet
        let mut write_buffer = Buffer::with_capacity(MTU_PACKET_BYTES);
        let mut writer = write_buffer.0.start_write();
        packet.encode(self.message_registry, &mut writer)?;
        let bytes = writer.finish_write();
        Ok(bytes)
    }

    /// Decode a packet from raw bytes
    // TODO: the reader buffer will be created from the io (we copy the io bytes into a buffer)
    pub(crate) fn decode_packet(&mut self, reader: &mut impl Read) -> anyhow::Result<Packet> {
        Packet::decode(self.message_registry, reader)
    }

    /// Start building new packet
    pub(crate) fn build_new_packet(&mut self) -> Packet {
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

    pub fn try_add_message(
        &mut self,
        packet: &mut Packet,
        message: MessageContainer,
    ) -> anyhow::Result<()> {
        match packet {
            Packet::Single(single_packet) => {
                let data = self.bytes_buffer.encode(&message)?;
                // TODO: create function can fit?
                if data.len() <= self.num_bytes_available {
                    single_packet.data.push(message);
                    Ok(())
                } else {
                    Err(anyhow!("Message too big to fit in packet"))
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

    #[test]
    fn test_write_small_message() -> anyhow::Result<()> {
        let mut manager = PacketManager::new();

        let small_message = MessageContainer::new(Bytes::from("small"));
        let mut packet = manager.build_new_packet();
        manager.try_add_message(&mut packet, small_message.clone())?;

        assert_eq!(packet.num_messages(), 1);

        manager.try_add_message(&mut packet, small_message.clone())?;
        assert_eq!(packet.num_messages(), 2);
        Ok(())
    }

    #[test]
    fn test_write_big_message() -> anyhow::Result<()> {
        let mut manager = PacketManager::new();

        let big_bytes = vec![1u8; 2 * MTU_PACKET_BYTES];
        let big_message = MessageContainer::new(Bytes::from(big_bytes));
        let mut packet = manager.build_new_packet();
        let error = manager
            .try_add_message(&mut packet, big_message)
            .unwrap_err();
        let root_cause = error.root_cause();
        assert_eq!(
            format!("{}", root_cause),
            "Message too big to fit in packet"
        );
        Ok(())
    }
}
