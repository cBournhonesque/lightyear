use std::collections::{BTreeMap, VecDeque};

use bitcode::encoding::Gamma;
use bitcode::word_buffer::WordBuffer;

use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::{FragmentData, MessageContainer, SingleData};
use crate::packet::packet::{
    FragmentedPacket, Packet, PacketData, SinglePacket, FRAGMENT_SIZE, MTU_PAYLOAD_BYTES,
};
use crate::packet::packet_type::PacketType;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::writer::WriteWordBuffer;
use crate::serialize::writer::WriteBuffer;

// enough to hold a biggest fragment + writing channel/message_id/etc.
// pub(crate) const PACKET_BUFFER_CAPACITY: usize = MTU_PAYLOAD_BYTES * (u8::BITS as usize) + 50;
pub(crate) const PACKET_BUFFER_CAPACITY: usize = MTU_PAYLOAD_BYTES * (u8::BITS as usize);

pub type Payload = Vec<u8>;

/// `PacketBuilder` handles the process of creating a packet (writing the header and packing the
/// messages into packets)
pub(crate) struct PacketBuilder {
    pub(crate) header_manager: PacketHeaderManager,
    // Pre-allocated buffer to encode/decode without allocation.
    // TODO: should this be associated with Packet?
    try_write_buffer: WriteWordBuffer,
    write_buffer: WriteWordBuffer,
}

impl PacketBuilder {
    pub fn new() -> Self {
        Self {
            header_manager: PacketHeaderManager::new(),
            // write buffer to encode packets bit by bit
            try_write_buffer: WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY),
            write_buffer: WriteBuffer::with_capacity(PACKET_BUFFER_CAPACITY),
        }
    }

    /// Reset the buffers used to encode packets
    pub fn clear_try_write_buffer(&mut self) {
        self.try_write_buffer.start_write();
        debug_assert_eq!(self.try_write_buffer.num_bits_written(), 0);
        // self.try_write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        self.try_write_buffer
            .set_reserved_bits(PACKET_BUFFER_CAPACITY);
    }

    //
    /// Reset the buffers used to encode packets
    pub fn clear_write_buffer(&mut self) {
        self.write_buffer.start_write();
        // self.write_buffer = WriteBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        self.write_buffer.set_reserved_bits(PACKET_BUFFER_CAPACITY);
    }

    /// Encode a packet into raw bytes
    pub(crate) fn encode_packet(&mut self, packet: &Packet) -> anyhow::Result<Payload> {
        // TODO: check that we haven't allocated!
        // self.clear_write_buffer();

        let mut write_buffer = WriteWordBuffer::with_capacity(PACKET_BUFFER_CAPACITY);
        write_buffer.set_reserved_bits(PACKET_BUFFER_CAPACITY);
        packet.encode(&mut write_buffer)?;
        // TODO: we should actually call finish write to byte align!
        // TODO: CAREFUL, THIS COULD ALLOCATE A BIT MORE TO BYTE ALIGN?
        let payload = Payload::from(write_buffer.finish_write());
        assert!(payload.len() <= MAX_PACKET_SIZE, "packet = {:?}", packet);
        Ok(payload)

        // packet.encode(&mut self.write_buffer)?;
        // let bytes = self.write_buffer.finish_write();
        // Ok(bytes)
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_single_packet(&mut self) -> Packet {
        self.clear_try_write_buffer();

        // NOTE: we assume that the header size is fixed, so we can just write PAYLOAD_BYTES
        //  if that's not the case we will need to serialize the header first
        // self.try_write_buffer
        //     .serialize(packet.header())
        //     .expect("Failed to serialize header, this should never happen");
        // TODO: need to reserver HEADER_BYTES bits?
        let header = self
            .header_manager
            .prepare_send_packet_header(PacketType::Data);
        Packet {
            header,
            data: PacketData::Single(SinglePacket::new()),
        }
    }

    pub(crate) fn build_new_fragment_packet(
        &mut self,
        channel_id: NetId,
        fragment_data: FragmentData,
    ) -> Packet {
        self.clear_try_write_buffer();

        // NOTE: we assume that the header size is fixed, so we can just write PAYLOAD_BYTES
        //  if that's not the case we will need to serialize the header first
        // self.try_write_buffer
        //     .serialize(packet.header())
        //     .expect("Failed to serialize header, this should never happen");
        let header = self
            .header_manager
            .prepare_send_packet_header(PacketType::DataFragment);
        let is_last_fragment = fragment_data.is_last_fragment();
        let packet = FragmentedPacket::new(channel_id, fragment_data);

        // TODO: how do we know how many bits are necessary to write the fragmented packet + bytes?
        //  - could try to compute it manually, but the length of Bytes is encoded with Gamma
        //  - could serialize the packet somewhere, and check the number of bits written

        debug_assert!(packet.fragment.bytes.len() <= FRAGMENT_SIZE);
        if is_last_fragment {
            packet.encode(&mut self.try_write_buffer).unwrap();
            // reserve one extra bit for the continuation bit between fragment/single packet data
            self.try_write_buffer.reserve_bits(1);

            // let num_bits_written = self.try_write_buffer.num_bits_written();
            // no need to reserve bits, since we already just wrote in the try buffer!
            // self.try_write_buffer.reserve_bits(num_bits_written);
            debug_assert!(!self.try_write_buffer.overflowed())
        }

        Packet {
            header,
            data: PacketData::Fragmented(packet),
        }

        // // fragments are 0-indexed, and for the last one we'll need to include the number of bytes as a u16
        // if fragment_id == num_fragments - 1 {
        //     self.try_write_buffer.reserve_bits(u16::BITS as usize);
        // }
        //
        // // each fragment will be byte-aligned
        // self.try_write_buffer.reserve_bits(bytes.len() * u8::BITS)
    }

    pub fn message_num_bits(&mut self, message: &MessageContainer) -> anyhow::Result<usize> {
        let mut write_buffer = WriteWordBuffer::with_capacity(2 * PACKET_BUFFER_CAPACITY);
        let prev_num_bits = write_buffer.num_bits_written();
        message.encode(&mut write_buffer)?;
        Ok(write_buffer.num_bits_written() - prev_num_bits)
    }

    pub fn can_add_message(&mut self, message: &SingleData) -> anyhow::Result<bool> {
        message.encode(&mut self.try_write_buffer)?;
        // reserve one extra bit for the MessageContinue bit
        self.try_write_buffer.reserve_bits(1);
        // TODO: we should release the bits if we don't end up writing the message;
        //  but it's not needed because if we can't write a message we start a new packet
        //  still it's dangerous
        Ok(!self.try_write_buffer.overflowed())
    }

    /// Returns true if there's enough space in the current packet to add a message
    /// The expectation is that we only work on a single packet at a time.
    pub fn can_add_bits(&mut self, num_bits: usize) -> bool {
        self.try_write_buffer.reserve_bits(num_bits + 1);
        !self.try_write_buffer.overflowed()
        // match packet {
        //     Packet::Single(single_packet) => {
        //         // TODO: either
        //         //  - get a function on the encoder that computes the amount of bits that the serialization will take
        //         //  - or we serialize and check the amount of bits it took
        //
        //         // // try to serialize in the try buffer
        //         // if message_num_bits > MTU_PAYLOAD_BYTES * 8 {
        //         //     panic!("Message too big to fit in packet")
        //         // }
        //
        //         // self.try_write_buffer.serialize(message)?;
        //         // reserve a MessageContinue bit associated with each Message.
        //         self.try_write_buffer.reserve_bits(num_bits + 1);
        //         !self.try_write_buffer.overflowed()
        //     }
        //     Packet::Fragmented(fragmented) => {
        //         self.try_write_buffer.reserve_bits(num_bits + )
        //     },
        // }
    }

    // TODO:
    // - we can set the priority on the channel level; then users can just create multiple channels
    // - we always send all messages for the same channel at the same time

    // - therefore, when a channel wants to pack messages, it ONLY WORKS IF CHANNELS ARE ITERATED IN ORDER
    // (i.e. we don't send channel 1, then channel 2, then channel 1)

    /// Try adding a channel to the current packet
    /// Returns false if there is not enough space left.
    /// If there is, we reserve the space for the channel in the try buffer.
    pub fn can_add_channel_to_packet(
        &mut self,
        channel_id: &NetId,
        packet: &mut Packet,
    ) -> anyhow::Result<bool> {
        // Reserve ChannelContinue bit, that indicates that whether or not there will be more
        // channels written in this packet
        self.try_write_buffer.encode(channel_id, Gamma)?;
        // self.try_write_buffer.serialize(channel_id)?;
        self.try_write_buffer.reserve_bits(1);
        if self.try_write_buffer.overflowed() {
            return Ok(false);
        }

        // Add a channel in the list of channels contained in the packet
        // (whether or not it will contain messages)
        packet.add_channel(*channel_id);
        Ok(true)
    }

    // /// Try to start writing for a new channel in the current packet
    // /// Reserving the correct amount of bits in the try buffer
    // /// Returns false if there is not enough space left
    // pub fn can_add_channel(&mut self, channel_kind: ChannelKind) -> anyhow::Result<bool> {
    //     // start building a new packet if necessary
    //     if self.current_packet.is_none() {
    //         return Ok(false);
    //     }
    //
    //     // Check if we have enough space to add the channel information
    //     self.current_channel = Some(channel_kind);
    //     // TODO: we could pass the channel registry as static to the buffers
    //     let net_id = self
    //         .channel_kind_map
    //         .net_id(&channel_kind)
    //         .context("Channel not found in registry")?;
    //     self.try_write_buffer.serialize(net_id)?;
    //
    //     // Reserve ChannelContinue bit, that indicates that whether or not there will be more
    //     // channels written in this packet
    //     self.try_write_buffer.reserve_bits(1);
    //     if self.try_write_buffer.overflowed() {
    //         return Ok(false);
    //     }
    //
    //     // Add a channel in the list of channels contained in the packet
    //     // (whether or not it will contain messages)
    //     self.current_packet
    //         .as_mut()
    //         .expect("No current packet being built")
    //         .add_channel(*net_id);
    //     Ok(true)
    // }

    // pub(crate) fn take_current_packet(&mut self) -> Option<Packet> {
    //     self.current_packet.take()
    // }

    // /// Get packets to be sent over the network, reset the internal buffer of packets to send
    // pub(crate) fn flush_packets(&mut self) -> Vec<Packet> {
    //     let mut packets = std::mem::take(&mut self.current_packets);
    //     if self.current_packet.is_some() {
    //         packets.push(std::mem::take(&mut self.current_packet).unwrap());
    //     }
    //     packets
    // }

    // pub(crate) fn fragment_message(
    //     &mut self,
    //     message: MessageContainer,
    //     message_num_bits: usize,
    // ) -> Vec<FragmentData> {
    //     let mut writer = WriteWordBuffer::with_capacity(message_num_bits);
    //     message.encode(&mut writer).unwrap();
    //     let raw_bytes = writer.finish_write();
    //     let chunks = raw_bytes.chunks(FRAGMENT_SIZE);
    //     let num_fragments = chunks.len();
    //
    //     chunks
    //         .enumerate()
    //         // TODO: ideally we don't clone here but we take ownership of the output of writer
    //         .map(|(fragment_index, chunk)| FragmentData {
    //             message_id: message.id().expect("Fragments need to have a message id"),
    //             fragment_id: fragment_index as u8,
    //             num_fragments: num_fragments as u8,
    //             bytes: Bytes::copy_from_slice(chunk),
    //         })
    //         .collect::<_>()
    // }

    pub fn build_packets(
        &mut self,
        // TODO: change into IntoIterator? the order matters though!
        data: BTreeMap<NetId, (VecDeque<SingleData>, VecDeque<FragmentData>)>,
    ) -> Vec<Packet> {
        let mut packets: Vec<Packet> = vec![];
        let mut single_packet: Option<Packet> = None;

        for (channel_id, (mut single_messages, fragment_messages)) in data.into_iter() {
            // sort from smallest to largest
            single_messages
                .make_contiguous()
                .sort_by_key(|message| message.bytes.len());

            // Finish writing the last single packet if need be
            if single_packet.is_some() {
                let mut packet = single_packet.take().unwrap();
                // add messages to packet for the given channel
                loop {
                    // no more messages to send, keep current packet for future messages from other channels
                    if single_messages.is_empty() {
                        single_packet = Some(packet);
                        break;
                    }

                    // TODO: bin packing, add the biggest message that could fit
                    //  use a free list of Option<SingleData> to keep track of which messages have been added?

                    // TODO: rename to can add message?
                    if self
                        .can_add_message(single_messages.front().unwrap())
                        .unwrap()
                    {
                        let message = single_messages.pop_front().unwrap();
                        // add message to packet
                        packet.add_message(channel_id, message);
                    } else {
                        // can't add any more messages (since we sorted messages from smallest to largest)
                        // finish packet
                        packets.push(packet);
                        break;
                    }
                }
            }

            // Start by writing all fragmented packets
            for fragment_data in fragment_messages.into_iter() {
                let is_last_fragment = fragment_data.is_last_fragment();
                debug_assert!(fragment_data.bytes.len() <= FRAGMENT_SIZE);
                let mut packet = self.build_new_fragment_packet(channel_id, fragment_data);
                if is_last_fragment {
                    loop {
                        // try to add single messages into the last fragment
                        if single_messages.is_empty() {
                            // if we were already building a single packet, finish it
                            // and make the current fragment packet the new 'current packet'
                            if let Some(single_packet) = single_packet {
                                packets.push(single_packet);
                            }
                            // keep this packet around for future channels
                            single_packet = Some(packet);
                            break;
                        }

                        // TODO: bin packing, add the biggest message that could fit
                        //  use a free list of Option<SingleData> to keep track of which messages have been added?
                        if self
                            .can_add_message(single_messages.front().unwrap())
                            .unwrap()
                        {
                            let message = single_messages.pop_front().unwrap();
                            // add message to packet
                            packet.add_message(channel_id, message);
                        } else {
                            // finish packet
                            packets.push(packet);
                            break;
                        }
                    }
                } else {
                    packets.push(packet);
                }
            }

            // Write any remaining single packets
            'packet: loop {
                // Can we write the channel id? If not, start a new packet (and add the channel id)
                if single_packet.is_none()
                    || single_packet
                        .as_mut()
                        .is_some_and(|p| !self.can_add_channel_to_packet(&channel_id, p).unwrap())
                {
                    let mut packet = self.build_new_single_packet();
                    // single_packet = Some(self.build_new_single_packet());
                    // add the channel to the new packet
                    self.can_add_channel_to_packet(&channel_id, &mut packet)
                        .unwrap();
                    single_packet = Some(packet);
                }

                let mut packet = single_packet.take().unwrap();
                // add messages to packet for the given channel
                'message: loop {
                    // no more messages to send, keep current packet for future messages from other channels
                    if single_messages.is_empty() {
                        single_packet = Some(packet);
                        break 'packet;
                    }

                    // TODO: bin packing, add the biggest message that could fit
                    //  use a free list of Option<SingleData> to keep track of which messages have been added?
                    if self
                        .can_add_message(single_messages.front().unwrap())
                        .unwrap()
                    {
                        let message = single_messages.pop_front().unwrap();
                        // add message to packet
                        packet.add_message(channel_id, message);
                    } else {
                        // can't add any more messages (since we sorted messages from smallest to largest)
                        // finish packet
                        packets.push(packet);
                        break 'message;
                    }
                }
            }
        }
        // if we had a packet we were working on, push it
        if let Some(packet) = single_packet {
            packets.push(packet);
        }
        packets
    }

    // /// Pack messages into packets for the current channel
    // /// Also return the remaining list of messages to send, as well the message ids of the messages
    // /// that were sent
    // ///
    // /// Uses First-fit-decreasing bin packing to store the messages in packets
    // /// https://en.wikipedia.org/wiki/First-fit-decreasing_bin_packing
    // /// (i.e. put the biggest message that fits in the packet)
    // pub fn pack_messages_within_channel(
    //     &mut self,
    //     mut single_messages_to_send: VecDeque<SingleData>,
    // ) -> (VecDeque<MessageContainer>, Vec<MessageId>) {
    //     // TODO: new impl
    //     //  - loop through messages. Any packets that are bigger than the MTU, we split them into fragments
    //     //  - we fill the last fragment piece with other messages
    //     //  - if its too big leave it for the end?
    //
    //     // TODO: where we do check for the available amount of bytes? here or in the channel sender?
    //
    //     // or should we split the messages into fragment right away?
    //
    //     // sort the values from smallest size to biggest
    //     // let mut messages_with_size = messages_to_send
    //     //     .into_iter()
    //     //     .map(|message| {
    //     //         let num_bits = self.message_num_bits(&message).unwrap();
    //     //         (message, num_bits)
    //     //     })
    //     //     .collect::<VecDeque<_>>();
    //     let mut messages = messages_to_send
    //         .into_iter()
    //         .map(|message| (Some(message), message.bytes().len(), message.is_fragment()))
    //         .collect::<VecDeque<_>>();
    //     // sort from largest to smallest
    //     messages
    //         .make_contiguous()
    //         .sort_by_key(|(_, size, _)| std::cmp::Reverse(*size));
    //
    //     // find the point where messages need to be fragmented
    //     // let partition_point = messages.partition_point(|(_, size)| *size > FRAGMENT_SIZE);
    //     let mut num_fragments = messages
    //         .iter()
    //         .filter(|(_, _, is_fragment)| *is_fragment)
    //         .count();
    //     let mut num_single_messages = messages_to_send.len() - num_fragments;
    //
    //     let mut sent_message_ids = Vec::new();
    //     // safety: we always start a new channel before we start building packets
    //     let channel = self.current_channel.unwrap();
    //     let channel_id = *self.channel_kind_map.net_id(&channel).unwrap();
    //
    //     // if there's a current packet being written, add single messages (from largest to smallest)
    //     // until we can't fit any more
    //     if self.current_packet.is_some() {
    //         let mut packet = self.current_packet.take().unwrap();
    //         if !packet.is_empty() {
    //             loop {
    //                 if num_single_messages == 0 {
    //                     break;
    //                 }
    //                 // find the next smallest
    //                 for i in (0..num_single_messages).rev() {}
    //                 let (message, size, is_fragment) = messages.back().unwrap();
    //                 // TODO: we might go multiple times over a fragment, optimize!!
    //                 if !*is_fragment {
    //                     break;
    //                 }
    //                 // TODO: use a better bin packing algorithm, putting the smallest message is not optimal
    //                 // TODO: make bin packing an option! if there are not many messages, or if we see right away
    //                 //  that they would all fit...
    //                 // messages_with_size.partition_point(|(_, size)| *size > FRAGMENT_SIZE);
    //
    //                 let (_, num_bits) = messages_with_size.front().unwrap();
    //                 if self.can_add_bits(*num_bits) {
    //                     let (message, _) = messages_with_size.pop_front().unwrap();
    //                     num_single_messages -= 1;
    //                     // add message to packet
    //                     if let Some(id) = message.id {
    //                         sent_message_ids.push(id);
    //                     }
    //                     packet.add_message(channel_id, message);
    //                 } else {
    //                     // finish packet
    //                     self.current_packets.push(packet);
    //                     break;
    //                 }
    //             }
    //         }
    //     }
    //
    //     // then start writing the fragmented packets, from biggest to smallest
    //     'packet: loop {
    //         // if self.current_packet.is_none() {
    //         //     self.build_new_packet();
    //         //     self.can_add_channel(channel).unwrap();
    //         // }
    //         // split the message into fragments
    //         if num_fragmented_messages == 0 {
    //             break 'packet;
    //         }
    //         let (fragment_message, num_bits) = messages_with_size.pop_back().unwrap();
    //         num_fragmented_messages -= 1;
    //         let all_fragment_data = self.fragment_message(fragment_message, num_bits);
    //         for fragment_data in all_fragment_data.into_iter() {
    //             let fragment_id = fragment_data.fragment_id;
    //             let num_fragments = fragment_data.num_fragments;
    //             self.build_new_fragment_packet(channel_id, fragment_data);
    //             self.can_add_channel(channel).unwrap();
    //             let mut packet = self.current_packet.take().unwrap();
    //
    //             // for the last fragment, add as many single messages as possible
    //             if fragment_id as u8 == num_fragments - 1 {
    //                 // TODO: remove duplicated code!
    //                 'message: loop {
    //                     if num_single_messages == 0 {
    //                         // no more messages to send, keep current packet buffer for future messages from other channels
    //                         self.current_packet = Some(packet);
    //                         break 'packet;
    //                     }
    //
    //                     // TODO: use a better bin packing algorithm, putting the smallest message is not optimal
    //                     let (_, num_bits) = messages_with_size.front().unwrap();
    //                     if self.can_add_bits(*num_bits) {
    //                         let (message, _) = messages_with_size.pop_front().unwrap();
    //                         num_single_messages -= 1;
    //                         // add message to packet
    //                         if let Some(id) = message.id {
    //                             sent_message_ids.push(id);
    //                         }
    //                         packet.add_message(channel_id, message);
    //                     } else {
    //                         // finish packet
    //                         self.current_packets.push(packet);
    //                         break 'message;
    //                     }
    //                 }
    //             }
    //         }
    //     }
    //
    //     // then write the remaining single messages that are left.
    //     // build new packet
    //     'packet: loop {
    //         // if it's a new packet, start by adding the channel
    //         if self.current_packet.is_none() {
    //             self.build_new_single_packet();
    //             self.can_add_channel(channel).unwrap();
    //         }
    //         let mut packet = self.current_packet.take().unwrap();
    //
    //         // add messages to packet for the given channel
    //         'message: loop {
    //             if num_single_messages == 0 {
    //                 // no more messages to send, keep current packet buffer for future messages from other channels
    //                 self.current_packet = Some(packet);
    //                 break 'packet;
    //             }
    //             // TODO: use a better bin packing algorithm, putting the smallest message is not optimal
    //             let (_, num_bits) = messages_with_size.front().unwrap();
    //             if self.can_add_bits(*num_bits) {
    //                 let (message, _) = messages_with_size.pop_front().unwrap();
    //                 num_single_messages -= 1;
    //                 // add message to packet
    //                 if let Some(id) = message.id {
    //                     sent_message_ids.push(id);
    //                 }
    //                 packet.add_message(channel_id, message);
    //             } else {
    //                 // finish packet
    //                 self.current_packets.push(packet);
    //                 break;
    //             }
    //         }
    //     }
    //     // remaining messages that were not added to packet
    //     let messages_to_send = messages_with_size
    //         .into_iter()
    //         .map(|(message, _)| message)
    //         .collect();
    //     (messages_to_send, sent_message_ids)
    // }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use bevy::prelude::default;
    use bytes::Bytes;

    use lightyear_macros::ChannelInternal;

    use crate::_reexport::*;
    use crate::channel::senders::fragment_sender::FragmentSender;
    use crate::packet::message::MessageId;
    use crate::prelude::*;

    use super::*;

    #[derive(ChannelInternal)]
    struct Channel1;

    #[derive(ChannelInternal)]
    struct Channel2;

    #[derive(ChannelInternal)]
    struct Channel3;

    fn get_channel_registry() -> ChannelRegistry {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        let mut c = ChannelRegistry::new();
        c.add::<Channel1>(settings.clone());
        c.add::<Channel2>(settings.clone());
        c.add::<Channel3>(settings.clone());
        c
    }

    #[test]
    fn test_write_small_message() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let small_message = Bytes::from("hello");
        let mut packet = manager.build_new_single_packet();
        assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);

        assert!(manager.can_add_bits(small_message.len() * (u8::BITS as usize)),);
        packet.add_message(
            *channel_id,
            SingleData::new(None, small_message.clone(), 1.0),
        );
        assert_eq!(packet.num_messages(), 1);

        assert!(manager.can_add_bits(small_message.len() * (u8::BITS as usize)),);
        packet.add_message(
            *channel_id,
            SingleData::new(None, small_message.clone(), 1.0),
        );
        assert_eq!(packet.num_messages(), 2);
        Ok(())
    }

    #[test]
    fn test_write_big_message() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();

        let big_message = Bytes::from(vec![1u8; 2 * MTU_PAYLOAD_BYTES]);
        let mut packet = manager.build_new_single_packet();
        assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);

        // the big message is too big to fit in the packet
        assert!(!manager.can_add_bits(big_message.len() * (u8::BITS as usize)),);
        Ok(())
    }

    #[test]
    fn test_pack_big_message() {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new();
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let num_big_bytes = (2.5 * MTU_PAYLOAD_BYTES as f32) as usize;
        let big_bytes = Bytes::from(vec![1u8; num_big_bytes]);
        let fragmenter = FragmentSender::new();
        let fragments = fragmenter.build_fragments(MessageId(0), None, big_bytes.clone(), 1.0);

        let small_bytes = Bytes::from(vec![0u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone(), 1.0);

        let mut data = BTreeMap::new();
        data.insert(
            *channel_id1,
            (VecDeque::from(vec![small_message.clone()]), VecDeque::new()),
        );
        data.insert(
            *channel_id2,
            (
                VecDeque::from(vec![small_message.clone()]),
                fragments.clone().into(),
            ),
        );
        data.insert(
            *channel_id3,
            (VecDeque::from(vec![small_message.clone()]), VecDeque::new()),
        );
        let mut packets = manager.build_packets(data);
        // we start building the packet for channel 1, we add one small message
        // we add one more small message to the packet from channel1, then we push fragments 1 and 2 for channel 2
        // we start working on fragment 3 for channel 2, and push the packet from channel 1 (with 2 messages)
        // then we push the small message from channel 3 into fragment 3
        assert_eq!(packets.len(), 4);
        let contents3 = packets.pop().unwrap().data.contents();
        assert_eq!(contents3.len(), 2);
        assert_eq!(
            contents3.get(channel_id2).unwrap(),
            &vec![fragments[2].clone().into()]
        );
        assert_eq!(
            contents3.get(channel_id3).unwrap(),
            &vec![small_message.clone().into()]
        );
        let contents2 = packets.pop().unwrap().data.contents();
        assert_eq!(contents2.len(), 2);
        assert_eq!(
            contents2.get(channel_id1).unwrap(),
            &vec![small_message.clone().into()]
        );
        assert_eq!(
            contents2.get(channel_id2).unwrap(),
            &vec![small_message.clone().into()]
        );
        let contents1 = packets.pop().unwrap().data.contents();
        assert_eq!(contents1.len(), 1);
        assert_eq!(
            contents1.get(channel_id2).unwrap(),
            &vec![fragments[1].clone().into()]
        );
        let contents0 = packets.pop().unwrap().data.contents();
        assert_eq!(contents0.len(), 1);
        assert_eq!(
            contents0.get(channel_id2).unwrap(),
            &vec![fragments[0].clone().into()]
        );
    }

    #[test]
    fn test_cannot_write_channel() -> anyhow::Result<()> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new();
        let channel_kind = ChannelKind::of::<Channel1>();
        let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
        let mut packet = manager.build_new_single_packet();

        // the channel_id takes only one bit to write (we use gamma encoding)
        // only 1 bit can be written
        manager.try_write_buffer.set_reserved_bits(1);
        // cannot write channel because of the continuation bit
        assert!(!manager.can_add_channel_to_packet(channel_id, &mut packet)?,);

        manager.clear_try_write_buffer();
        manager.try_write_buffer.set_reserved_bits(2);
        assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);
        Ok(())
    }

    // #[test]
    // fn test_write_pack_messages_in_multiple_packets() -> anyhow::Result<()> {
    //     let channel_registry = get_channel_registry();
    //     let mut manager = PacketManager::new(channel_registry.kind_map());
    //     let channel_kind = ChannelKind::of::<Channel1>();
    //     let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
    //
    //     let mut message0 = Bytes::from(vec![false; MTU_PAYLOAD_BYTES - 100]);
    //     message0.set_id(MessageId(0));
    //     let mut message1 = Bytes::from(vec![true; MTU_PAYLOAD_BYTES - 100]);
    //     message1.set_id(MessageId(1));
    //
    //     let mut packet = manager.build_new_packet();
    //     assert_eq!(manager.can_add_channel(channel_kind)?, true);
    //
    //     // 8..16 take 7 bits with gamma encoding
    //     let messages: VecDeque<_> = vec![message0, message1].into();
    //     let (remaining_messages, sent_message_ids) = manager.pack_messages_within_channel(messages);
    //
    //     let packets = manager.flush_packets();
    //     assert_eq!(packets.len(), 2);
    //     assert_eq!(remaining_messages.is_empty(), true);
    //     assert_eq!(sent_message_ids, vec![MessageId(0), MessageId(1)]);
    //
    //     Ok(())
    // }
}
