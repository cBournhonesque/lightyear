//! Module to take a buffer of messages to send and build packets
use byteorder::WriteBytesExt;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Write};
#[cfg(feature = "trace")]
use tracing::{instrument, Level};

use bitcode::encoding::Gamma;

use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::{FragmentData, MessageAck, MessageContainer, MessageId, SingleData};
use crate::packet::packet::{
    FragmentedPacket, Packet, PacketData, SinglePacket, FRAGMENT_SIZE, MTU_PAYLOAD_BYTES,
};
use crate::packet::packet_type::PacketType;
use crate::prelude::Tick;
use crate::protocol::channel::ChannelId;
use crate::protocol::registry::NetId;
use crate::protocol::BitSerializable;
use crate::serialize::bitcode::writer::BitcodeWriter;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::serialize::{SerializationError, ToBytes};

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
    cursor: Vec<u8>,
    acks: Vec<(ChannelId, Vec<MessageAck>)>,
    prewritten_size: usize,
    mid_packet: bool,
}

impl PacketBuilder {
    pub fn new(nack_rtt_multiple: f32) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(nack_rtt_multiple),
            cursor: Vec::with_capacity(PACKET_BUFFER_CAPACITY),
            acks: Vec::new(),
            // we start with 1 extra byte for the final ChannelId = 0 that marks the end of the packet
            prewritten_size: 1,
            // are we in the middle of writing a packet?
            mid_packet: false,
        }
    }

    /// Reset the buffers used to encode packets
    pub fn clear_cursor(&mut self) {
        self.cursor = Vec::with_capacity(MTU_PAYLOAD_BYTES);
        self.prewritten_size = 1;
        self.mid_packet = false;
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_single_packet(
        &mut self,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        // self.clear_cursor();

        // write the header
        let mut header = self
            .header_manager
            .prepare_send_packet_header(PacketType::Data);
        header.tick = current_tick;
        header.to_bytes(&mut self.cursor)?;
        Ok(())
    }

    pub(crate) fn build_new_fragment_packet(
        &mut self,
        channel_id: NetId,
        fragment_data: &FragmentData,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        // self.clear_cursor();
        let mut header = self
            .header_manager
            .prepare_send_packet_header(PacketType::DataFragment);
        header.tick = current_tick;
        header.to_bytes(&mut self.cursor)?;
        channel_id.to_bytes(&mut self.cursor)?;
        fragment_data.to_bytes(&mut self.cursor)?;
        self.acks.push((
            channel_id,
            vec![MessageAck {
                message_id: fragment_data.message_id,
                fragment_id: Some(fragment_data.fragment_id),
            }],
        ));
        Ok(())

        //
        // let is_last_fragment = fragment_data.is_last_fragment();
        // let packet = FragmentedPacket::new(channel_id, fragment_data);
        //
        // debug_assert!(packet.fragment.bytes.len() <= FRAGMENT_SIZE);
        // if is_last_fragment {
        //     packet.encode(&mut self.try_write_buffer).unwrap();
        //     // reserve one extra bit for the continuation bit between fragment/single packet data
        //     self.try_write_buffer.reserve_bits(1);
        //
        //     // let num_bits_written = self.try_write_buffer.num_bits_written();
        //     // no need to reserve bits, since we already just wrote in the try buffer!
        //     // self.try_write_buffer.reserve_bits(num_bits_written);
        //     debug_assert!(!self.try_write_buffer.overflowed())
        // }
        //
        // Packet {
        //     header,
        //     data: PacketData::Fragmented(packet),
        // }
    }

    /// Check that we can still fit some data in the buffer
    pub fn can_fit(&mut self, size: usize) -> bool {
        self.cursor.capacity() >= size + self.prewritten_size + self.cursor.len()
    }

    pub fn finish_packet(&mut self) -> Packet {
        self.cursor.shrink_to_fit();
        // TODO: should we use bytes so this clone is cheap?
        let payload = self.cursor.clone();
        self.clear_cursor();
        Packet {
            payload,
            message_acks: std::mem::take(&mut self.acks),
        }
    }

    pub fn build_packets(
        &mut self,
        current_tick: Tick,
        data: BTreeMap<ChannelId, (VecDeque<SingleData>, VecDeque<FragmentData>)>,
    ) -> Result<Vec<Packet>, SerializationError> {
        let mut packets: Vec<Packet> = vec![];
        let write_single_messages = |cursor: &mut Vec<u8>,
                                     messages: &VecDeque<SingleData>,
                                     start: &mut usize,
                                     end: &mut usize,
                                     channel_id: ChannelId|
         -> Result<(), SerializationError> {
            channel_id.to_bytes(cursor)?;
            // write the number of messages for the current channel
            let num_messages = *end - *start;
            if num_messages > 0 {
                cursor.write_u8(num_messages as u8).unwrap();
                // write the messages
                let mut message_acks = vec![];
                for i in *start..*end {
                    messages[i].to_bytes(cursor).unwrap();
                    message_acks.push(MessageAck {
                        // TODO: REVISIT THIS
                        message_id: messages[i].id.unwrap_or(MessageId(0)),
                        fragment_id: None,
                    });
                }
                self.acks.push((channel_id, message_acks));
                *start = *end;
            }

            Ok(())
        };

        'outer: for (channel_id, (mut single_messages, fragment_messages)) in data.into_iter() {
            // index (inclusive) of the first message that hasn't been written yet but that we will write
            let mut message_start_idx = 0;
            // index (exclusive) of the last message that hasn't been written yet but that we will write
            let mut message_end_idx = 0;
            // sort from smallest to largest
            single_messages
                .make_contiguous()
                .sort_by_key(|message| message.bytes.len());

            // Finish writing single_messages in the current packet if need be
            if self.mid_packet {
                // add messages to packet for the given channel
                loop {
                    // no more messages to send in this channel, try to fill with messages from other channels
                    if message_end_idx == single_messages.len() {
                        continue 'outer;
                    }

                    // TODO: bin packing, add the biggest message that could fit
                    //  use a free list of Option<SingleData> to keep track of which messages have been added?

                    // TODO: rename to can add message?
                    if self.can_fit(single_messages[message_end_idx].len()) {
                        message_end_idx += 1;
                        self.prewritten_size += single_messages[message_end_idx].len();
                    } else {
                        // can't add any more messages (since we sorted messages from smallest to largest)
                        // finish packet and start a new one
                        write_single_messages(
                            &mut self.cursor,
                            &single_messages,
                            &mut message_start_idx,
                            &mut message_end_idx,
                            channel_id,
                        )?;
                        packets.push(self.finish_packet());
                        break;
                    }
                }
            }

            // Start by writing all fragmented packets
            for fragment_data in fragment_messages {
                debug_assert!(fragment_data.bytes.len() <= FRAGMENT_SIZE);
                self.build_new_fragment_packet(channel_id, &fragment_data, current_tick)?;
                // if it's the last fragment, we can try to fill it with small messages
                // TODO: is this a good idea? does it break some reliability guarantees?
                if fragment_data.is_last_fragment() {
                    loop {
                        // try to add single messages into the last fragment
                        if message_end_idx == single_messages.len() {
                            // go back to the top of the loop to add more single messages to this packet
                            continue 'outer;
                        }

                        // TODO: bin packing, add the biggest message that could fit
                        //  use a free list of Option<SingleData> to keep track of which messages have been added?

                        if self.can_fit(single_messages[message_end_idx].len()) {
                            message_end_idx += 1;
                            self.prewritten_size += single_messages[message_end_idx].len();
                        } else {
                            // can't add any more messages (since we sorted messages from smallest to largest)
                            // finish packet and start a new one
                            write_single_messages(
                                &mut self.cursor,
                                &single_messages,
                                &mut message_start_idx,
                                &mut message_end_idx,
                                channel_id,
                            )?;
                            packets.push(self.finish_packet());
                            break;
                        }
                    }
                } else {
                    packets.push(self.finish_packet());
                }
            }

            // Write any remaining single packets
            'packet: loop {
                // Can we write the channel id + num messages? If not, start a new packet (and write the channel id)
                if !self.mid_packet && !self.can_fit(channel_id.len() + 1) {
                    self.build_new_single_packet(current_tick)?;
                }

                // add messages to packet for the given channel
                // we won't add the messages directly, we will just get the indices of the messages we need to write
                // (because we need to know the total count of messages first so that we can write it right after the
                // the channel id)
                'message: loop {
                    // no more messages to send in this channel!
                    // write all the messages that we kept track of
                    // keep current packet for messages from other channels
                    if message_end_idx == single_messages.len() {
                        write_single_messages(
                            &mut self.cursor,
                            &single_messages,
                            &mut message_start_idx,
                            &mut message_end_idx,
                            channel_id,
                        )?;
                        // go to next channel
                        break 'packet;
                    }

                    // TODO: bin packing, add the biggest message that could fit
                    //  use a free list of Option<SingleData> to keep track of which messages have been added?
                    if self.can_fit(single_messages[message_end_idx].len()) {
                        message_end_idx += 1;
                        self.prewritten_size += single_messages[message_end_idx].len();
                    } else {
                        // can't add any more messages (since we sorted messages from smallest to largest)
                        // write messages, finish packet and start a new one
                        write_single_messages(
                            &mut self.cursor,
                            &single_messages,
                            &mut message_start_idx,
                            &mut message_end_idx,
                            channel_id,
                        )?;
                        packets.push(self.finish_packet());
                        break 'message;
                    }
                }
            }
        }
        if self.mid_packet {
            packets.push(self.finish_packet());
        }
        Ok(packets)
    }

    // /// Uses multiple exponential searches to fill a packet. Has a good worst case runtime and doesn't
    // /// create any extraneous extension packets.
    // fn pack_multiple_exponential(mut messages: &[Message]) -> Vec<Packet> {
    //     /// A Vec<u8> prefixed by its length as a u32. Each [`Packet`] contains 1 or more [`Section`]s.
    //     struct Section(Vec<u8>);
    //     impl Section {
    //         fn len(&self) -> usize {
    //             self.0.len() + std::mem::size_of::<u32>()
    //         }
    //         fn write(&self, out: &mut Vec<u8>) {
    //             out.reserve(self.len());
    //             out.extend_from_slice(&u32::try_from(self.0.len()).unwrap().to_le_bytes()); // TODO use varint.
    //             out.extend_from_slice(&self.0);
    //         }
    //     }
    //
    //     let mut buffer = bitcode::Buffer::new(); // TODO save between calls.
    //     let mut packets = vec![];
    //
    //     while !messages.is_empty() {
    //         let mut remaining = Packet::MAX_SIZE;
    //         let mut bytes = vec![];
    //
    //         while remaining > 0 && !messages.is_empty() {
    //             let mut i = 0;
    //             let mut previous = None;
    //
    //             loop {
    //                 i = (i * 2).clamp(1, messages.len());
    //                 const COMPRESS: bool = true;
    //                 let b = Section(if COMPRESS {
    //                     lz4_flex::compress_prepend_size(&buffer.encode(&messages[..i]))
    //                 } else {
    //                     buffer.encode(&messages[..i]).to_vec()
    //                 });
    //
    //                 let (i, b) = if b.len() <= remaining {
    //                     if i == messages.len() {
    //                         // No more messages.
    //                         (i, b)
    //                     } else {
    //                         // Try to fit more.
    //                         previous = Some((i, b));
    //                         continue;
    //                     }
    //                 } else if let Some((i, b)) = previous {
    //                     // Current failed, so use previous.
    //                     (i, b)
    //                 } else {
    //                     assert_eq!(i, 1);
    //                     // 1 message doesn't fit. If starting a new packet would result in fewer
    //                     // fragments, flush the current packet.
    //                     let flush_fragments = b.len().div_ceil(Packet::MAX_SIZE) - 1;
    //                     let keep_fragments = (b.len() - remaining).div_ceil(Packet::MAX_SIZE);
    //                     if flush_fragments < keep_fragments {
    //                         // TODO try to fill current packet by with packets after the single large packet.
    //                         packets.push(Packet(std::mem::take(&mut bytes)));
    //                         remaining = Packet::MAX_SIZE;
    //                     }
    //                     (i, b)
    //                 };
    //
    //                 messages = &messages[i..];
    //                 if bytes.is_empty() && b.len() < Packet::MAX_SIZE {
    //                     bytes = Vec::with_capacity(Packet::MAX_SIZE); // Assume we'll fill the packet.
    //                 }
    //                 b.write(&mut bytes);
    //                 if b.len() > remaining {
    //                     assert_eq!(i, 1);
    //                     // TODO fill extension packets. We would need to know where the section ends
    //                     // within the packet in case previous packets are lost.
    //                     remaining = 0;
    //                 } else {
    //                     remaining -= b.len();
    //                 }
    //                 break;
    //             }
    //         }
    //         packets.push(Packet(bytes));
    //     }
    //     packets
    // }
}
//
// #[cfg(test)]
// mod tests {
//     use std::collections::{BTreeMap, VecDeque};
//
//     use bevy::prelude::{default, TypePath};
//     use bytes::Bytes;
//
//     use lightyear_macros::ChannelInternal;
//
//     use crate::channel::senders::fragment_sender::FragmentSender;
//     use crate::packet::message::MessageId;
//     use crate::prelude::*;
//
//     use super::*;
//
//     #[derive(ChannelInternal, TypePath)]
//     struct Channel1;
//
//     #[derive(ChannelInternal, TypePath)]
//     struct Channel2;
//
//     #[derive(ChannelInternal, TypePath)]
//     struct Channel3;
//
//     fn get_channel_registry() -> ChannelRegistry {
//         let settings = ChannelSettings {
//             mode: ChannelMode::UnorderedUnreliable,
//             ..default()
//         };
//         let mut c = ChannelRegistry::default();
//         c.add_channel::<Channel1>(settings.clone());
//         c.add_channel::<Channel2>(settings.clone());
//         c.add_channel::<Channel3>(settings.clone());
//         c
//     }
//
//     #[test]
//     fn test_write_small_message() -> anyhow::Result<()> {
//         let channel_registry = get_channel_registry();
//         let mut manager = PacketBuilder::new(1.5);
//         let channel_kind = ChannelKind::of::<Channel1>();
//         let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
//
//         let small_message = Bytes::from("hello");
//         let mut packet = manager.build_new_single_packet();
//         assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);
//
//         assert!(manager.can_add_bits(small_message.len() * (u8::BITS as usize)),);
//         packet.add_message(
//             *channel_id,
//             SingleData::new(None, small_message.clone(), 1.0),
//         );
//         assert_eq!(packet.num_messages(), 1);
//
//         assert!(manager.can_add_bits(small_message.len() * (u8::BITS as usize)),);
//         packet.add_message(
//             *channel_id,
//             SingleData::new(None, small_message.clone(), 1.0),
//         );
//         assert_eq!(packet.num_messages(), 2);
//         Ok(())
//     }
//
//     #[test]
//     fn test_write_big_message() -> anyhow::Result<()> {
//         let channel_registry = get_channel_registry();
//         let mut manager = PacketBuilder::new(1.5);
//         let channel_kind = ChannelKind::of::<Channel1>();
//         let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
//
//         let big_message = Bytes::from(vec![1u8; 2 * MTU_PAYLOAD_BYTES]);
//         let mut packet = manager.build_new_single_packet();
//         assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);
//
//         // the big message is too big to fit in the packet
//         assert!(!manager.can_add_bits(big_message.len() * (u8::BITS as usize)),);
//         Ok(())
//     }
//
//     #[test]
//     fn test_pack_big_message() {
//         let channel_registry = get_channel_registry();
//         let mut manager = PacketBuilder::new(1.5);
//         let channel_kind1 = ChannelKind::of::<Channel1>();
//         let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
//         let channel_kind2 = ChannelKind::of::<Channel2>();
//         let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
//         let channel_kind3 = ChannelKind::of::<Channel3>();
//         let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();
//
//         let num_big_bytes = (2.5 * MTU_PAYLOAD_BYTES as f32) as usize;
//         let big_bytes = Bytes::from(vec![1u8; num_big_bytes]);
//         let fragmenter = FragmentSender::new();
//         let fragments = fragmenter.build_fragments(MessageId(0), None, big_bytes.clone(), 1.0);
//
//         let small_bytes = Bytes::from(vec![0u8; 10]);
//         let small_message = SingleData::new(None, small_bytes.clone(), 1.0);
//
//         let mut data = BTreeMap::new();
//         data.insert(
//             *channel_id1,
//             (VecDeque::from(vec![small_message.clone()]), VecDeque::new()),
//         );
//         data.insert(
//             *channel_id2,
//             (
//                 VecDeque::from(vec![small_message.clone()]),
//                 fragments.clone().into(),
//             ),
//         );
//         data.insert(
//             *channel_id3,
//             (VecDeque::from(vec![small_message.clone()]), VecDeque::new()),
//         );
//         let mut packets = manager.build_packets(data);
//         // we start building the packet for channel 1, we add one small message
//         // we add one more small message to the packet from channel1, then we push fragments 1 and 2 for channel 2
//         // we start working on fragment 3 for channel 2, and push the packet from channel 1 (with 2 messages)
//         // then we push the small message from channel 3 into fragment 3
//         assert_eq!(packets.len(), 4);
//         let contents3 = packets.pop().unwrap().data.contents();
//         assert_eq!(contents3.len(), 2);
//         assert_eq!(
//             contents3.get(channel_id2).unwrap(),
//             &vec![fragments[2].clone().into()]
//         );
//         assert_eq!(
//             contents3.get(channel_id3).unwrap(),
//             &vec![small_message.clone().into()]
//         );
//         let contents2 = packets.pop().unwrap().data.contents();
//         assert_eq!(contents2.len(), 2);
//         assert_eq!(
//             contents2.get(channel_id1).unwrap(),
//             &vec![small_message.clone().into()]
//         );
//         assert_eq!(
//             contents2.get(channel_id2).unwrap(),
//             &vec![small_message.clone().into()]
//         );
//         let contents1 = packets.pop().unwrap().data.contents();
//         assert_eq!(contents1.len(), 1);
//         assert_eq!(
//             contents1.get(channel_id2).unwrap(),
//             &vec![fragments[1].clone().into()]
//         );
//         let contents0 = packets.pop().unwrap().data.contents();
//         assert_eq!(contents0.len(), 1);
//         assert_eq!(
//             contents0.get(channel_id2).unwrap(),
//             &vec![fragments[0].clone().into()]
//         );
//     }
//
//     #[test]
//     fn test_cannot_write_channel() -> anyhow::Result<()> {
//         let channel_registry = get_channel_registry();
//         let mut manager = PacketBuilder::new(1.5);
//         let channel_kind = ChannelKind::of::<Channel1>();
//         let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
//         let mut packet = manager.build_new_single_packet();
//
//         // the channel_id takes only one bit to write (we use gamma encoding)
//         // only 1 bit can be written
//         manager.try_write_buffer.set_reserved_bits(1);
//         // cannot write channel because of the continuation bit
//         assert!(!manager.can_add_channel_to_packet(channel_id, &mut packet)?,);
//
//         manager.clear_try_write_buffer();
//         manager.try_write_buffer.set_reserved_bits(2);
//         assert!(manager.can_add_channel_to_packet(channel_id, &mut packet)?,);
//         Ok(())
//     }
//
//     // #[test]
//     // fn test_write_pack_messages_in_multiple_packets() -> anyhow::Result<()> {
//     //     let channel_registry = get_channel_registry();
//     //     let mut manager = PacketManager::new(channel_registry.kind_map());
//     //     let channel_kind = ChannelKind::of::<Channel1>();
//     //     let channel_id = channel_registry.get_net_from_kind(&channel_kind).unwrap();
//     //
//     //     let mut message0 = Bytes::from(vec![false; MTU_PAYLOAD_BYTES - 100]);
//     //     message0.set_id(MessageId(0));
//     //     let mut message1 = Bytes::from(vec![true; MTU_PAYLOAD_BYTES - 100]);
//     //     message1.set_id(MessageId(1));
//     //
//     //     let mut packet = manager.build_new_packet();
//     //     assert_eq!(manager.can_add_channel(channel_kind)?, true);
//     //
//     //     // 8..16 take 7 bits with gamma encoding
//     //     let messages: VecDeque<_> = vec![message0, message1].into();
//     //     let (remaining_messages, sent_message_ids) = manager.pack_messages_within_channel(messages);
//     //
//     //     let packets = manager.flush_packets();
//     //     assert_eq!(packets.len(), 2);
//     //     assert_eq!(remaining_messages.is_empty(), true);
//     //     assert_eq!(sent_message_ids, vec![MessageId(0), MessageId(1)]);
//     //
//     //     Ok(())
//     // }
// }
