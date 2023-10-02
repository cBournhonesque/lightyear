use std::collections::VecDeque;

use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::write::Write;

use crate::packet::header::PacketHeaderManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::{Packet, SinglePacket, MTU_PACKET_BYTES};
use crate::packet::packet_type::PacketType;
use crate::protocol::{Protocol, SerializableProtocol};
use crate::registry::channel::ChannelRegistry;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::ChannelKind;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = 1 * MTU_PACKET_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager {
    pub(crate) header_manager: PacketHeaderManager,
    num_bits_available: usize,
    // TODO: maybe need Arc<> here?
    channel_registry: &'static ChannelRegistry,
    // current_packet?
    /// Current channel that is being written
    current_channel: Option<ChannelKind>,
    /// Pre-allocated buffer to encode/decode without allocation.
    try_write_buffer: WriteWordBuffer,
    write_buffer: WriteWordBuffer,
}

// PLAN:
// -

impl PacketManager {
    pub fn new(channel_registry: &'static ChannelRegistry) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            num_bits_available: MTU_PACKET_BYTES * 8,
            channel_registry,
            current_channel: None,
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
    pub(crate) fn encode_packet<P: SerializableProtocol>(
        &mut self,
        packet: &Packet<P>,
    ) -> anyhow::Result<&[u8]> {
        // TODO: check that we haven't allocated!

        // Create a write buffer with capacity the size of a packet
        // let mut write_buffer = Buffer::with_capacity(MTU_PACKET_BYTES);
        // let mut writer = write_buffer.0.start_write();
        packet.encode(&mut self.write_buffer)?;
        let bytes = self.write_buffer.finish_write();
        // let bytes = write_buffer.0.finish_write(writer);
        Ok(bytes)
    }

    /// Decode a packet from raw bytes
    // TODO: the reader buffer will be created from the io (we copy the io bytes into a buffer)
    pub(crate) fn decode_packet<P: SerializableProtocol>(
        &mut self,
        reader: &mut impl ReadBuffer,
    ) -> anyhow::Result<Packet<P>> {
        Packet::decode(reader)
    }

    /// Start building new packet
    pub(crate) fn build_new_packet<P>(&mut self) -> Packet<P> {
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
    pub fn can_add_message<P: SerializableProtocol>(
        &mut self,
        packet: &mut Packet<P>,
        message: &MessageContainer<P>,
    ) -> anyhow::Result<bool> {
        match packet {
            Packet::Single(single_packet) => {
                // TODO: either
                //  - get a function on the encoder that computes the amount of bits that the serialization will take
                //  - or we serialize and check the amount of bits it took

                // try to serialize in the try buffer
                let num_bits = message.encode(&mut self.try_write_buffer)?;
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

    // TODO:
    // - we want the packet manager to handle the channels used as well
    // - we want messages from multiple channels in the same packet
    // - we can set the priority on the channel level; then users can just create multiple channels
    // - we always send all messages for the same channel at the same time

    // - therefore, when a channel wants to pack messages, it ONLY WORKS IF CHANNELS ARE ITERATED IN ORDER
    // (i.e. we don't send channel 1, then channel 2, then channel 1)

    /// Pack messages into packets for the current channel
    /// Return the remaining list of messages to send
    pub fn pack_messages<P: SerializableProtocol>(
        &mut self,
        mut messages_to_send: VecDeque<MessageContainer<P>>,
    ) -> (Vec<Packet<P>>, VecDeque<MessageContainer<P>>) {
        let mut packets = Vec::new();
        // build new packet
        'packet: loop {
            let mut packet = self.build_new_packet();

            // add messages to packet
            'message: loop {
                // TODO: check if message size is too big for a single packet, in which case we fragment!
                if messages_to_send.is_empty() {
                    // no more messages to send, add the packet
                    packets.push(packet);
                    break 'packet;
                }
                // we're either moving the message into the packet, or back into the messages_to_send queue
                let message = messages_to_send.pop_front().unwrap();
                if self.can_add_message(&mut packet, &message).is_ok_and(|b| b) {
                    // add message to packet
                    packet.add_message(message);
                } else {
                    // message was not added to packet, packet is full
                    messages_to_send.push_front(message);
                    packets.push(packet);
                    break 'message;
                }
            }
        }
        (packets, messages_to_send)
    }
}

#[cfg(test)]
mod tests {
    use lazy_static::lazy_static;

    use lightyear_derive::ChannelInternal;

    use crate::packet::manager::PacketManager;
    use crate::packet::packet::MTU_PACKET_BYTES;
    use crate::{
        ChannelDirection, ChannelMode, ChannelRegistry, ChannelSettings, MessageContainer,
    };

    #[derive(ChannelInternal)]
    struct Channel1;

    lazy_static! {
        static ref CHANNEL_REGISTRY: ChannelRegistry = {
            let settings = ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                direction: ChannelDirection::Bidirectional,
            };
            let mut c = ChannelRegistry::new();
            c.add::<Channel1>(settings).unwrap();
            c
        };
    }

    #[test]
    fn test_write_small_message() -> anyhow::Result<()> {
        let mut manager = PacketManager::new(&CHANNEL_REGISTRY);

        let small_message = MessageContainer::new(0);
        let mut packet = manager.build_new_packet();
        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(small_message.clone());
        assert_eq!(packet.num_messages(), 1);

        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(small_message.clone());
        assert_eq!(packet.num_messages(), 2);
        Ok(())
    }

    #[test]
    fn test_write_big_message() -> anyhow::Result<()> {
        let mut manager = PacketManager::new(&CHANNEL_REGISTRY);

        let big_bytes = vec![1u8; 2 * MTU_PACKET_BYTES];
        let big_message = MessageContainer::new(big_bytes);
        let mut packet = manager.build_new_packet();
        assert_eq!(manager.can_add_message(&mut packet, &big_message)?, false);
        // let error = manager
        //     .can_add_message(&mut packet, big_message)
        //     .unwrap_err();
        // let root_cause = error.root_cause();
        // assert_eq!(
        //     format!("{}", root_cause),
        //     "Message too big to fit in packet"
        // );
        Ok(())
    }

    // #[test]
    // fn test_write_big_message() -> anyhow::Result<()> {
    //     let mut manager = PacketManager::new(&CHANNEL_REGISTRY);
    // }
}
