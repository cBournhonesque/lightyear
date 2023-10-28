use std::collections::VecDeque;

use anyhow::Context;
use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::write::Write;
use bytes::Bytes;

use crate::packet::header::PacketHeaderManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::{
    FragmentedPacket, Packet, SinglePacket, FRAGMENT_SIZE, MTU_PAYLOAD_BYTES,
};
use crate::packet::wrapping_id::MessageId;
use crate::protocol::registry::TypeMapper;
use crate::protocol::{BitSerializable, Protocol};
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::ChannelKind;

pub(crate) const PACKET_BUFFER_CAPACITY: usize = MTU_PAYLOAD_BYTES;

/// Handles the process of sending and receiving packets
pub(crate) struct PacketManager<P: BitSerializable> {
    pub(crate) header_manager: PacketHeaderManager,
    pub(crate) channel_kind_map: TypeMapper<ChannelKind>,

    /// Current packets that have been built but must be sent over the network
    /// (excludes current_packet)
    current_packets: Vec<Packet<P>>,
    /// Current packet that is being written
    pub(crate) current_packet: Option<Packet<P>>,
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

impl<P: BitSerializable> PacketManager<P> {
    pub fn new(channel_kind_map: TypeMapper<ChannelKind>) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            channel_kind_map,
            current_packets: Vec::new(),
            current_packet: None,
            current_channel: None,
            /// write buffer to encode packets bit by bit
            // TODO: create a BufWriter to keep track of both the buffer and the Writer. 
            try_write_buffer: WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY),
            write_buffer: WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    /// Reset the buffers used to encode packets
    pub fn clear_try_write_buffer(&mut self) {
        self.try_write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        self.try_write_buffer
            .set_reserved_bits(PACKET_BUFFER_CAPACITY);
    }

    //
    /// Reset the buffers used to encode packets
    pub fn clear_write_buffer(&mut self) {
        self.write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        self.write_buffer.set_reserved_bits(PACKET_BUFFER_CAPACITY);
    }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet<P>) -> anyhow::Result<impl WriteBuffer> {
        // TODO: check that we haven't allocated!
        // self.clear_write_buffer();

        let mut write_buffer = WriteWordBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        write_buffer.set_reserved_bits(PACKET_BUFFER_CAPACITY);
        packet.encode(&mut write_buffer)?;
        Ok(write_buffer)

        // packet.encode(&mut self.write_buffer)?;
        // let bytes = self.write_buffer.finish_write();
        // Ok(bytes)
    }

    /// Decode a packet from raw bytes
    // TODO: the reader buffer will be created from the io (we copy the io bytes into a buffer)
    // Should we decode the packet and get ChannelKinds directly?
    pub(crate) fn decode_packet(
        &mut self,
        reader: &mut impl ReadBuffer,
    ) -> anyhow::Result<Packet<P>> {
        Packet::<P>::decode(reader)
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_packet(&mut self) {
        self.clear_try_write_buffer();

        // NOTE: we assume that the header size is fixed, so we can just write PAYLOAD_BYTES
        //  if that's not the case we will need to serialize the header first
        // self.try_write_buffer
        //     .serialize(packet.header())
        //     .expect("Failed to serialize header, this should never happen");
        self.current_packet = Some(Packet::Single(SinglePacket::new(&mut self)));
    }

    pub(crate) fn build_new_fragment_packet(
        &mut self,
        fragment_id: u8,
        num_fragments: u8,
        bytes: Bytes,
    ) {
        self.clear_try_write_buffer();

        // NOTE: we assume that the header size is fixed, so we can just write PAYLOAD_BYTES
        //  if that's not the case we will need to serialize the header first
        // self.try_write_buffer
        //     .serialize(packet.header())
        //     .expect("Failed to serialize header, this should never happen");
        self.current_packet = Some(Packet::Fragmented(FragmentedPacket::new(
            &mut self,
            fragment_id,
            num_fragments,
            bytes,
        )));
        // fragments are 0-indexed, and for the last one we'll need to include the number of bytes as a u16
        if fragment_id == num_fragments - 1 {
            self.try_write_buffer.reserve_bits(u16::BITS as usize);
        }

        // each fragment will be byte-aligned
        self.try_write_buffer.reserve_bits(bytes.len() * u8::BITS)
    }

    pub fn message_num_bits(&mut self, message: &MessageContainer<P>) -> anyhow::Result<usize> {
        let mut write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        let prev_num_bits = write_buffer.num_bits_written();
        message.encode(&mut write_buffer)?;
        Ok(write_buffer.num_bits_written() - prev_num_bits)
    }

    /// Returns true if there's enough space in the current packet to add a message
    /// The expectation is that we only work on a single packet at a time.
    pub fn can_add_bits(&mut self, packet: &mut Packet<P>, num_bits: usize) -> bool {
        match packet {
            Packet::Single(single_packet) => {
                // TODO: either
                //  - get a function on the encoder that computes the amount of bits that the serialization will take
                //  - or we serialize and check the amount of bits it took

                // // try to serialize in the try buffer
                // if message_num_bits > MTU_PAYLOAD_BYTES * 8 {
                //     panic!("Message too big to fit in packet")
                // }

                // self.try_write_buffer.serialize(message)?;
                // reserve a MessageContinue bit associated with each Message.
                self.try_write_buffer.reserve_bits(num_bits + 1);
                !self.try_write_buffer.overflowed()
            }
            _ => unimplemented!(),
        }
    }

    // TODO:
    // - we can set the priority on the channel level; then users can just create multiple channels
    // - we always send all messages for the same channel at the same time

    // - therefore, when a channel wants to pack messages, it ONLY WORKS IF CHANNELS ARE ITERATED IN ORDER
    // (i.e. we don't send channel 1, then channel 2, then channel 1)

    /// Try to start writing for a new channel in the current packet
    /// Reserving the correct amount of bits in the try buffer
    /// Returns false if there is not enough space left
    pub fn can_add_channel(&mut self, channel_kind: ChannelKind) -> anyhow::Result<bool> {
        // start building a new packet if necessary
        if self.current_packet.is_none() {
            return Ok(false);
        }

        // Check if we have enough space to add the channel information
        self.current_channel = Some(channel_kind);
        // TODO: we could pass the channel registry as static to the buffers
        let net_id = self
            .channel_kind_map
            .net_id(&channel_kind)
            .context("Channel not found in registry")?;
        self.try_write_buffer.serialize(net_id)?;

        // Reserve ChannelContinue bit, that indicates that whether or not there will be more
        // channels written in this packet
        self.try_write_buffer.reserve_bits(1);
        if self.try_write_buffer.overflowed() {
            return Ok(false);
        }

        // Add a channel in the list of channels contained in the packet
        // (whether or not it will contain messages)
        self.current_packet
            .as_mut()
            .expect("No current packet being built")
            .add_channel(*net_id);
        Ok(true)
    }

    pub(crate) fn take_current_packet(&mut self) -> Option<Packet<P>> {
        self.current_packet.take()
    }

    /// Get packets to be sent over the network, reset the internal buffer of packets to send
    pub(crate) fn flush_packets(&mut self) -> Vec<Packet<P>> {
        let mut packets = std::mem::take(&mut self.current_packets);
        if self.current_packet.is_some() {
            packets.push(std::mem::take(&mut self.current_packet).unwrap());
        }
        packets
    }

    pub(crate) fn fragment_message(
        &mut self,
        message: MessageContainer<P>,
        message_num_bits: usize,
    ) -> Vec<Bytes> {
        let mut writer = WriteBuffer::with_capacity(message_num_bits);
        message.encode(&mut writer).unwrap();
        let bytes = Bytes::from(writer.finish_write());
        bytes.chunks(FRAGMENT_SIZE).collect::<_>()
    }

    /// Pack messages into packets for the current channel
    /// Also return the remaining list of messages to send, as well the message ids of the messages
    /// that were sent
    pub fn pack_messages_within_channel(
        &mut self,
        mut messages_to_send: VecDeque<MessageContainer<P>>,
    ) -> (VecDeque<MessageContainer<P>>, Vec<MessageId>) {
        // TODO: new impl
        //  - loop through messages. Any packets that are bigger than the MTU, we split them into fragments
        //  - we fill the last fragment piece with other messages
        //  - if its too big leave it for the end?

        // sort the values from biggest size to smallest
        let mut messages_with_size = messages_to_send
            .into_iter()
            .map(|message| {
                let num_bits = self.message_num_bits(&message).unwrap();
                (message, num_bits)
            })
            .collect::<VecDeque<_>>();
        // sort in descending order of message size
        messages_with_size.sort_by_key(|(_, size)| -size);
        // messages_with_size.iter().partition()
        let partition_point = messages_with_size.partition_point(|(_, size)| *size > FRAGMENT_SIZE);
        // let (fragment_messages, single_messages) = messages_with_size.into_iter().partition(|(_, size)| *size > FRAGMENT_SIZE);
        let mut num_fragmented_messages = partition_point;
        let mut num_single_messages = messages_with_size.len() - partition_point;

        // // all messages that have to be fragmented
        // let mut fragment_messages =  &messages_with_size[..partition_point];
        // // messages that can be sent in a single packet
        // let mut single_messages = &messages_with_size[partition_point..];

        // SHOULD WE DO BIN PACKING?
        let mut sent_message_ids = Vec::new();
        // safety: we always start a new channel before we start building packets
        let channel = self.current_channel.unwrap();
        let channel_id = *self.channel_kind_map.net_id(&channel).unwrap();

        // if there's a current packet being written, add single messages from smallest to biggest
        // until we can't fit any more
        if self.current_packet.is_some() {
            let Some(mut packet) = self.current_packet.take();
            if !packet.is_empty() {
                loop {
                    if num_single_messages == 0 {
                        break;
                    }
                    let (message, num_bits) = messages_with_size.pop_back().unwrap();
                    // TODO: use a better bin packing algorithm, putting the smallest message is not optimal
                    if self.can_add_bits(&mut packet, num_bits) {
                        // add message to packet
                        if let Some(id) = message.id {
                            sent_message_ids.push(id);
                        }
                        packet.add_message(channel_id, message.clone());
                    } else {
                        // packet is too big
                        break;
                    }
                }
            }
        }

        // then start writing the fragmented packets, from biggest to smallest
        'packet: loop {
            // if self.current_packet.is_none() {
            //     self.build_new_packet();
            //     self.can_add_channel(channel).unwrap();
            // }
            // split the message into fragments
            if num_fragmented_messages == 0 {
                break 'packet;
            }
            let (fragment_message, num_bits) = messages_with_size.pop_front().unwrap();
            let all_fragment_bytes = self.fragment_message(fragment_message, num_bits);
            let num_fragments = all_fragment_bytes.len() as u8;

            for (fragment_index, fragment_bytes) in all_fragment_bytes.iter().enumerate() {
                self.build_new_fragment_packet(
                    fragment_index as u8,
                    num_fragments,
                    fragment_bytes.clone(),
                );
                self.can_add_channel(channel).unwrap();
                let mut packet = self.current_packet.take().unwrap();

                // for the last fragment, add as many single messages as possible
                if fragment_index == num_fragments - 1 {
                    // TODO: remove duplicated code!
                    'message: loop {
                        if num_single_messages == 0 {
                            break 'message;
                        }
                        let (message, num_bits) = messages_with_size.pop_back().unwrap();
                        // TODO: use a better bin packing algorithm, putting the smallest message is not optimal
                        if self.can_add_bits(&mut packet, num_bits) {
                            // add message to packet
                            if let Some(id) = message.id {
                                sent_message_ids.push(id);
                            }
                            packet.add_message(channel_id, message.clone());
                        } else {
                            // packet is too big
                            break 'message;
                        }
                    }
                }
            }
        }

        // then write the remaining single messages that are left.
        // TODO TOMORROW!!!!!!!!

        // build new packet
        'packet: loop {
            // if it's a new packet, start by adding the channel
            if self.current_packet.is_none() {
                self.build_new_packet();
                self.can_add_channel(channel).unwrap();
            }
            let mut packet = self.current_packet.take().unwrap();

            // add messages to packet for the given channel
            'message: loop {
                if messages_to_send.is_empty() {
                    // TODO: send warning about message being too big?
                    // no more messages to send, keep current packet buffer for future messages
                    self.current_packet = Some(packet);
                    break 'packet;
                }

                // we're either moving the message into the packet, or back into the messages_to_send queue
                let message = messages_to_send.pop_front().unwrap();

                // TODO: check if message size is too big for a single packet, in which case we fragment!
                if self.can_add_message(&mut packet, &message).is_ok_and(|b| b) {
                    // add message to packet
                    if let Some(id) = message.id {
                        sent_message_ids.push(id);
                    }
                    packet.add_message(channel_id, message);
                } else {
                    // TODO: should we order messages by size to fit the smallest messages first?
                    //  or by size + priority + order?

                    // message was not added to packet, packet is full
                    messages_to_send.push_front(message);
                    self.current_packets.push(packet);
                    break 'message;
                }
            }
        }
        (messages_to_send, sent_message_ids)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bitvec::access::BitAccess;

    use lightyear_derive::ChannelInternal;

    use crate::packet::packet::MTU_PAYLOAD_BYTES;
    use crate::packet::packet_manager::PacketManager;
    use crate::packet::wrapping_id::MessageId;
    use crate::{
        ChannelDirection, ChannelKind, ChannelMode, ChannelRegistry, ChannelSettings,
        MessageContainer, WriteBuffer,
    };

    #[derive(ChannelInternal)]
    struct Channel1;

    #[derive(ChannelInternal)]
    struct Channel2;

    fn get_channel_registry() -> ChannelRegistry {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        };
        let mut c = ChannelRegistry::new();
        c.add::<Channel1>(settings.clone());
        c.add::<Channel2>(settings.clone());
        c
    }

    #[test]
    fn test_write_small_message() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let small_message = MessageContainer::new(0);
        manager.build_new_packet();
        assert_eq!(manager.can_add_channel(channel_kind)?, true);

        let mut packet = manager.current_packet.take().unwrap();
        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(channel_id.clone(), small_message.clone());
        assert_eq!(packet.num_messages(), 1);

        assert_eq!(manager.can_add_message(&mut packet, &small_message)?, true);
        packet.add_message(channel_id.clone(), small_message.clone());
        assert_eq!(packet.num_messages(), 2);
        Ok(())
    }

    #[test]
    fn test_write_big_message() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let big_bytes = vec![1u8; 2 * MTU_PAYLOAD_BYTES];
        let big_message = MessageContainer::new(big_bytes);
        manager.build_new_packet();
        assert_eq!(manager.can_add_channel(channel_kind)?, true);

        let mut packet = manager.current_packet.take().unwrap();
        // the big message is too big to fit in the packet
        assert_eq!(manager.can_add_message(&mut packet, &big_message)?, false);
        Ok(())
    }

    #[test]
    #[should_panic]
    fn test_pack_big_message() {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let big_bytes = vec![1u8; 10 * MTU_PAYLOAD_BYTES];
        let big_message = MessageContainer::new(big_bytes);
        manager.build_new_packet();
        manager.can_add_channel(channel_kind);
        manager.pack_messages_within_channel(vec![big_message].into());
    }

    #[test]
    fn test_cannot_write_channel() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::<u8>::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        manager.build_new_packet();

        // only 1 bit can be written
        manager.try_write_buffer.set_reserved_bits(1);
        // cannot write channel because of the continuation bit
        assert_eq!(manager.can_add_channel(channel_kind)?, false);

        manager.clear_try_write_buffer();
        manager.try_write_buffer.set_reserved_bits(2);
        assert_eq!(manager.can_add_channel(channel_kind)?, true);
        Ok(())
    }

    #[test]
    fn test_write_pack_messages_in_multiple_packets() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketManager::new(channel_registry.kind_map());
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let mut message0 = MessageContainer::new(vec![false; MTU_PAYLOAD_BYTES - 100]);
        message0.set_id(MessageId(0));
        let mut message1 = MessageContainer::new(vec![true; MTU_PAYLOAD_BYTES - 100]);
        message1.set_id(MessageId(1));

        let mut packet = manager.build_new_packet();
        assert_eq!(manager.can_add_channel(channel_kind)?, true);

        // 8..16 take 7 bits with gamma encoding
        let messages: VecDeque<_> = vec![message0, message1].into();
        let (remaining_messages, sent_message_ids) = manager.pack_messages_within_channel(messages);

        let packets = manager.flush_packets();
        assert_eq!(packets.len(), 2);
        assert_eq!(remaining_messages.is_empty(), true);
        assert_eq!(sent_message_ids, vec![MessageId(0), MessageId(1)]);

        Ok(())
    }
}
