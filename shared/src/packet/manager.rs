use crate::channel::channel::{ChannelHeader, ChannelKind};
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::Message;
use crate::packet::packet::{Packet, SinglePacket, MTU_PACKET_BYTES};
use crate::packet::packet_type::PacketType;
use anyhow::anyhow;
use bitcode::Buffer;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = 10 * MTU_PACKET_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    pub(crate) header_manager: PacketHeaderManager,
    num_bytes_available: usize,
    /// Pre-allocated buffer to encode/decode without allocation
    bytes_buffer: Buffer,
}

// PLAN:
// -

impl PacketManager {
    pub fn new() -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            num_bytes_available: MTU_PACKET_BYTES,
            bytes_buffer: Buffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    // /// Returns true if the given number of bits can fit into the current packet
    // fn can_fit(&self, num_bytes: u32) -> bool {
    //     num_bytes <= self.num_bytes_available
    // }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet) -> anyhow::Result<&[u8]> {
        self.bytes_buffer.encode(packet).map_err(|e| anyhow!(e))
    }

    /// Decode a packet from raw bytes
    pub(crate) fn decode_packet(&mut self, bytes: &[u8]) -> anyhow::Result<Packet> {
        self.bytes_buffer.decode(bytes).map_err(|e| anyhow!(e))
    }

    /// Start building new packet
    pub(crate) fn build_new_packet(&mut self) -> Packet {
        Packet::Single(SinglePacket {
            // TODO: handle protocol and packet type
            header: self.header_manager.prepare_send_packet_header(
                0,
                PacketType::Data,
                ChannelHeader {
                    kind: ChannelKind::new(0),
                },
            ),
            data: vec![],
        })
    }

    pub fn try_add_message(&mut self, packet: &mut Packet, message: Message) -> anyhow::Result<()> {
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
    use crate::packet::message::Message;
    use crate::packet::packet::MTU_PACKET_BYTES;
    use anyhow::anyhow;
    use bytes::Bytes;

    #[test]
    fn test_write_small_message() -> anyhow::Result<()> {
        let mut manager = PacketManager::new();

        let small_message = Message::new(Bytes::from("small"));
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
        let big_message = Message::new(Bytes::from(big_bytes));
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
