use crate::packet::header::PacketHeaderManager;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    header_manager: PacketHeaderManager,
}

pub trait PacketWriter {}

pub trait PacketReceiver {}

impl PacketManager {
    /// Returns true if the given number of bits can fit into the packet
    pub fn can_fit(&self, num_bits: u32) -> bool {
        unimplemented!()
    }

    /// Start building new packet
    pub fn build_new_packet(&mut self) -> Packet {
        Packet::Single(SinglePacket {
            // TODO: handle protocol and packet type
            header: self
                .packet_header_manager
                .prepare_send_packet_header(0, PacketType::Data),
            data: vec![],
        })
    }

    pub fn try_add_message(
        &mut self,
        packet: &mut Packet,
        message_id: MessageId,
        message: Message,
    ) -> anyhow::Result<()> {
        match packet {
            Packet::Single(single_packet) => {
                if self.can_fit(message.bit_len()) {
                    single_packet.data.push((message_id, message));
                    Ok(())
                } else {
                    Err(anyhow!("Message too big to fit in packet"))
                }
            }
            _ => unimplemented!(),
        }
    }
}
