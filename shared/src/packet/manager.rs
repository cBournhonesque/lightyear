use std::collections::VecDeque;

use anyhow::anyhow;
use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::write::Write;

use crate::packet::header::PacketHeaderManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::{Packet, MTU_PAYLOAD_BYTES};
use crate::packet::wrapping_id::MessageId;
use crate::protocol::{Protocol, SerializableProtocol};
use crate::registry::channel::ChannelRegistry;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::ChannelKind;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = 1 * MTU_PAYLOAD_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager<P: SerializableProtocol> {
    pub(crate) header_manager: PacketHeaderManager,
    num_bits_available: usize,
    // TODO: maybe need Arc<> here?
    channel_registry: &'static ChannelRegistry,

    /// Current packet that is being written
    current_packet: Option<Packet<P>>,
    /// Current channel that is being written
    current_channel: Option<ChannelKind>,
    /// Pre-allocated buffer to encode/decode without allocation.
    try_write_buffer: WriteWordBuffer,
    write_buffer: WriteWordBuffer,
}

// PLAN:
// renet version:
// - all types of messages we need to send are stored in the MessageRegistry and are encoded
// into Bytes very early in the process. This solves the problem of `dyn Message` because
// all the code just deals with Bytes.
// The MessageContainer just stores Bytes along with the kind of the message.
// At the very end of the code, we deserialize using the kind of message + the bytes?

impl<P: SerializableProtocol> PacketManager<P> {
    pub fn new(channel_registry: &'static ChannelRegistry) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            num_bits_available: MTU_PAYLOAD_BYTES * 8,
            channel_registry,
            current_packet: None,
            current_channel: None,
            /// write buffer to encode packets bit by bit
            // TODO: create a BufWriter to keep track of both the buffer and the Writer. 
            try_write_buffer: WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY),
            write_buffer: WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    /// Reset the buffers used to encode packets
    pub fn clear_write_buffers(&mut self) {
        self.try_write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        self.write_buffer = WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY);
        self.try_write_buffer.reserve_bits(PACKET_BUFFER_CAPACITY);
    }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet<P>) -> anyhow::Result<&[u8]> {
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
    pub(crate) fn decode_packet(
        &mut self,
        reader: &mut impl ReadBuffer,
    ) -> anyhow::Result<Packet<P>> {
        Packet::<P>::decode(reader)
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_packet(&mut self) {
        self.clear_write_buffers();
        self.current_packet = Some(Packet::new(self));
        // start writing the current channel

        //     bytes: []
        //     // TODO: handle protocol and packet type
        //     header: self
        //         .header_manager
        //         .prepare_send_packet_header(0, PacketType::Data),
        //     data: vec![],
        // }))
    }

    /// Returns true if there's enough space in the current packet to add a message
    /// The expectation is that we only work on a single packet at a time.
    pub fn can_add_message(
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
                message.encode(&mut self.try_write_buffer)?;
                // self.try_write_buffer.serialize(message)?;
                // reserve a MessageContinue bit associated with each Message.
                self.try_write_buffer.reserve_bits(1);
                if self.try_write_buffer.overflowed() {
                    Ok(false)
                } else {
                    Ok(true)
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

    /// Try to start writing messages for a new channel within the current packet.
    /// Reserving the correct amount of bits in the try buffer
    /// Returns false if there is not enough space left
    pub fn can_start_new_channel(&mut self, channel_kind: ChannelKind) -> anyhow::Result<bool> {
        self.current_channel = Some(channel_kind);
        // TODO: we could pass the channel registry as static to the buffers
        let net_id = self
            .channel_registry
            .get_net_from_kind(&channel_kind)
            .ok_or(anyhow!("Channel not found in registry"))?;
        self.try_write_buffer.serialize(net_id)?;

        // Reserve ChannelContinue bit, that indicates that whether or not there will be more
        // channels written in this packet
        self.try_write_buffer.reserve_bits(1);
        if self.try_write_buffer.overflowed() {
            return Ok(false);
        }

        // self.write_buffer.serialize(net_id)?;
        Ok(true)
    }

    pub(crate) fn take_current_packet(&mut self) -> Option<Packet<P>> {
        self.current_packet.take()
    }

    /// Pack messages into packets for the current channel
    /// Also return the remaining list of messages to send, as well the message ids of the messages
    /// that were sent
    pub fn pack_messages_within_channel(
        &mut self,
        mut messages_to_send: VecDeque<MessageContainer<P>>,
    ) -> (
        Vec<Packet<P>>,
        VecDeque<MessageContainer<P>>,
        Vec<MessageId>,
    ) {
        let mut packets = Vec::new();
        let mut sent_message_ids = Vec::new();

        // safety: we always start a new channel before we start building packets
        let channel = self.current_channel.unwrap();
        let channel_id = self
            .channel_registry
            .get_net_from_kind(&channel)
            .unwrap()
            .clone();

        // build new packet
        'packet: loop {
            if self.current_packet.is_none() {
                self.build_new_packet();
                self.can_start_new_channel(channel).unwrap();
            }
            let mut packet = self.current_packet.take().unwrap();

            // add messages to packet for the given channel
            'message: loop {
                // TODO: check if message size is too big for a single packet, in which case we fragment!
                if messages_to_send.is_empty() {
                    // TODO: send warning about message being too big?

                    // no more messages to send, add the packet
                    packets.push(packet);
                    break 'packet;
                }

                // we're either moving the message into the packet, or back into the messages_to_send queue
                let message = messages_to_send.pop_front().unwrap();
                if self.can_add_message(&mut packet, &message).is_ok_and(|b| b) {
                    // add message to packet
                    if let Some(id) = message.id.clone() {
                        sent_message_ids.push(id);
                    }
                    packet.add_message(channel_id, message);
                } else {
                    // TODO: should we order messages by size to fit the smallest messages first?
                    //  or by size + priority + order?

                    // message was not added to packet, packet is full
                    messages_to_send.push_front(message);
                    packets.push(packet);
                    break 'message;
                }
            }
        }
        (packets, messages_to_send, sent_message_ids)
    }
}

#[cfg(test)]
mod tests {
    use lazy_static::lazy_static;

    use lightyear_derive::ChannelInternal;

    use crate::packet::manager::PacketManager;
    use crate::{
        ChannelDirection, ChannelKind, ChannelMode, ChannelRegistry, ChannelSettings,
        MessageContainer,
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
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = CHANNEL_REGISTRY.get_net_from_kind(&channel_kind).unwrap();

        let small_message = MessageContainer::new(0);
        manager.build_new_packet();
        let mut packet = manager.current_packet.take().unwrap();
        assert_eq!(manager.can_start_new_channel(channel_kind)?, true);

        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(channel_id.clone(), small_message.clone());
        assert_eq!(packet.num_messages(), 1);

        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(channel_id.clone(), small_message.clone());
        assert_eq!(packet.num_messages(), 2);
        Ok(())
    }

    // #[test]
    // fn test_write_big_message() -> anyhow::Result<()> {
    //     let mut manager = PacketManager::new(&CHANNEL_REGISTRY);
    //
    //     let big_bytes = vec![1u8; 2 * MTU_PAYLOAD_BYTES];
    //     let big_message = MessageContainer::new(big_bytes);
    //     let mut packet = manager.build_new_packet();
    //     assert_eq!(manager.can_add_message(&mut packet, &big_message)?, false);
    //     // let error = manager
    //     //     .can_add_message(&mut packet, big_message)
    //     //     .unwrap_err();
    //     // let root_cause = error.root_cause();
    //     // assert_eq!(
    //     //     format!("{}", root_cause),
    //     //     "Message too big to fit in packet"
    //     // );
    //     Ok(())
    // }
    //
    // #[test]
    // fn test_write_big_message() -> anyhow::Result<()> {
    //     let mut manager = PacketManager::new(&CHANNEL_REGISTRY);
    // }
}
