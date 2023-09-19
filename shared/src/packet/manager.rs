use crate::channel::channel::{ChannelHeader, ChannelKind};
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::Message;
use crate::packet::packet::{Packet, SinglePacket};
use crate::packet::packet_type::PacketType;
use anyhow::anyhow;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    pub(crate) header_manager: PacketHeaderManager,
}

// PLAN:
// -

impl PacketManager {
    pub fn new() -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
        }
    }

    /// Returns true if the given number of bits can fit into the packet
    fn can_fit(&self, num_bits: u32) -> bool {
        unimplemented!()
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
                if self.can_fit(message.bit_len()) {
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
