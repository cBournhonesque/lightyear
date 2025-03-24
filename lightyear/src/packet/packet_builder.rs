//! Module to take a buffer of messages to send and build packets
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::packet::header::PacketHeaderManager;
use crate::packet::message::{FragmentData, MessageAck, SingleData};
use crate::packet::packet::{Packet, FRAGMENT_SIZE};
use crate::packet::packet_type::PacketType;
use crate::prelude::Tick;
use crate::protocol::channel::ChannelId;
use crate::protocol::registry::NetId;
use crate::serialize::varint::varint_len;
use crate::serialize::{SerializationError, ToBytes};
use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bytes::Bytes;
use tracing::trace;
#[cfg(feature = "trace")]
use tracing::{instrument, Level};
use crate::serialize::writer::WriteInteger;

pub type Payload = Vec<u8>;

/// We use `Bytes` on the receive side because we want to be able to refer to sub-slices of the original
/// packet without allocating.
///
/// e.g. we receive 1200 bytes from the network, we want to read parts of it (header, channel) but then
/// store subslices in receiver channels without allocating.
pub type RecvPayload = Bytes;

/// `PacketBuilder` handles the process of creating a packet (writing the header and packing the
/// messages into packets)
#[derive(Debug)]
pub(crate) struct PacketBuilder {
    pub(crate) header_manager: PacketHeaderManager,
    current_packet: Option<Packet>,
    // Pre-allocated buffer to encode/decode without allocation.
    // TODO: should this be associated with Packet?
    // cursor: Vec<u8>,
    // acks: Vec<(ChannelId, Vec<MessageAck>)>,
    // How many bytes we know we are going to have to write in the packet, but haven't written yet
    // prewritten_size: usize,
    // mid_packet: bool,
}

impl PacketBuilder {
    pub fn new(nack_rtt_multiple: f32) -> Self {
        Self {
            header_manager: PacketHeaderManager::new(nack_rtt_multiple),
            current_packet: None,
            // cursor: Vec::with_capacity(PACKET_BUFFER_CAPACITY),
            // acks: Vec::new(),

            // we start with 1 extra byte for the final ChannelId = 0 that marks the end of the packet
            // prewritten_size: 0,
            // are we in the middle of writing a packet?
            // mid_packet: false,
        }
    }

    // TODO: get the vec from a pool of preallocated buffers
    fn get_new_buffer(&self) -> Payload {
        Vec::with_capacity(MAX_PACKET_SIZE)
    }

    /// Start building new packet, we start with an empty packet
    /// that can write to a given channel
    pub(crate) fn build_new_single_packet(
        &mut self,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        let mut cursor = self.get_new_buffer();

        // write the header
        let mut header = self
            .header_manager
            .prepare_send_packet_header(PacketType::Data);
        // set the tick at which the packet will be sent
        header.tick = current_tick;
        header.to_bytes(&mut cursor)?;
        self.current_packet = Some(Packet {
            payload: cursor,
            message_acks: vec![],
            packet_id: header.packet_id,
            prewritten_size: 0,
        });
        Ok(())
    }

    pub(crate) fn build_new_fragment_packet(
        &mut self,
        channel_id: NetId,
        fragment_data: &FragmentData,
        current_tick: Tick,
    ) -> Result<(), SerializationError> {
        let mut cursor = self.get_new_buffer();
        // writer the header
        let mut header = self
            .header_manager
            .prepare_send_packet_header(PacketType::DataFragment);
        // set the tick at which the packet will be sent
        header.tick = current_tick;
        header.to_bytes(&mut cursor)?;
        channel_id.to_bytes(&mut cursor)?;
        fragment_data.to_bytes(&mut cursor)?;
        self.current_packet = Some(Packet {
            payload: cursor,
            // TODO: reuse this vec allocation instead of newly allocating!
            message_acks: vec![(
                ChannelId::from(channel_id),
                MessageAck {
                    message_id: fragment_data.message_id,
                    fragment_id: Some(fragment_data.fragment_id),
                },
            )],
            packet_id: header.packet_id,
            prewritten_size: 0,
        });
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

    pub fn finish_packet(&mut self) -> Packet {
        let mut packet = self.current_packet.take().unwrap();
        packet.payload.shrink_to_fit();
        // TODO: should we use bytes so this clone is cheap?
        packet
    }

    /// Pack messages into packets
    ///
    /// In general the strategy is:
    /// - sort the single data messages from smallest to largest
    /// - write the fragment data first. Big fragments take the entire packet. Small fragments have
    ///   some room to spare for small messages
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn build_packets(
        &mut self,
        current_tick: Tick,
        mut single_data: Vec<(ChannelId, VecDeque<SingleData>)>,
        fragment_data: Vec<(ChannelId, VecDeque<FragmentData>)>,
    ) -> Result<Vec<Packet>, SerializationError> {
        let mut packets: Vec<Packet> = vec![];

        // indices in the main vec
        let mut single_data_idx = 0;

        for (_, single_messages) in single_data.iter_mut() {
            // sort from smallest to largest each array of small messages
            single_messages
                .make_contiguous()
                .sort_by_key(|message| message.bytes.len());
        }

        // try to fill the packet with fragment messages first
        for (channel_id, mut fragment_messages) in fragment_data.into_iter() {
            while let Some(fragment_data) = fragment_messages.pop_front() {
                debug_assert!(fragment_data.bytes.len() <= FRAGMENT_SIZE);
                self.build_new_fragment_packet(channel_id, &fragment_data, current_tick)?;
                if !fragment_data.is_last_fragment() {
                    // big fragment, write packet immediately
                    packets.push(self.finish_packet());
                } else {
                    let mut packet = self.current_packet.take().unwrap();
                    // it's a smaller fragment, fill it with small messages
                    'out: while single_data_idx < single_data.len() {
                        // if we don't even have space for a new channel, return the packet immediately
                        if !packet.can_fit_channel(channel_id) {
                            break;
                        }

                        let (channel_id, single_messages) = &mut single_data[single_data_idx];
                        // number of messages for this channel that we will write
                        // (we wait until we know the full number, because we want to write that)
                        let mut num_messages = 0;
                        // fill with messages from the current channel
                        loop {
                            // no more messages to send in this channel, try to fill with messages from the next channels
                            if num_messages == single_messages.len() {
                                Self::write_single_messages(
                                    &mut packet,
                                    single_messages,
                                    &mut num_messages,
                                    *channel_id,
                                )?;
                                single_data_idx += 1;
                                break;
                            }

                            if packet.can_fit(single_messages[num_messages].bytes_len()) {
                                packet.prewritten_size += single_messages[num_messages].bytes_len();
                                num_messages += 1;
                            } else {
                                // can't add any more messages (since we sorted messages from smallest to largest)
                                // finish packet and go back to trying to write fragment messages
                                Self::write_single_messages(
                                    &mut packet,
                                    single_messages,
                                    &mut num_messages,
                                    *channel_id,
                                )?;
                                break 'out;
                            }
                        }
                    }
                    // no more single messages to send, finish the fragment packet
                    self.current_packet = Some(packet);
                    packets.push(self.finish_packet());
                }
            }
        }

        debug_assert!(self.current_packet.is_none());

        // all fragment messages have been written, now write small messages
        'out: while single_data_idx < single_data.len() {
            let (channel_id, single_messages) = &mut single_data[single_data_idx];
            // start a new packet if we aren't already writing one
            if self.current_packet.is_none() {
                self.build_new_single_packet(current_tick)?;
            }

            let mut packet = self.current_packet.take().unwrap();
            // we need to call this to preassign the channel_id
            if !packet.can_fit_channel(*channel_id) {
                // can't add any more messages (since we sorted messages from smallest to largest)
                // finish packet and go back to trying to write fragment messages
                self.current_packet = Some(packet);
                packets.push(self.finish_packet());
                continue 'out;
            }
            // number of messages for this channel that we will write
            // (we wait until we know the full number, because we want to write that)
            let mut num_messages = 0;
            // fill with messages from the current channel
            loop {
                // no more messages to send in this channel, try to fill with messages from the next channels
                if num_messages == single_messages.len() {
                    Self::write_single_messages(
                        &mut packet,
                        single_messages,
                        &mut num_messages,
                        *channel_id,
                    )?;
                    // we make sure we keep writing the current packet
                    self.current_packet = Some(packet);
                    single_data_idx += 1;
                    break;
                }

                if packet.can_fit(single_messages[num_messages].bytes_len()) {
                    packet.prewritten_size += single_messages[num_messages].bytes_len();
                    num_messages += 1;
                } else {
                    // can't add any more messages (since we sorted messages from smallest to largest)
                    // finish packet and go back to trying to write fragment messages
                    Self::write_single_messages(
                        &mut packet,
                        single_messages,
                        &mut num_messages,
                        *channel_id,
                    )?;
                    self.current_packet = Some(packet);
                    packets.push(self.finish_packet());
                    continue 'out;
                }
            }
        }

        // if we had a packet we were working on, push it
        if self.current_packet.is_some() {
            packets.push(self.finish_packet());
        }
        Ok(packets)
    }

    /// Helper function to fill the current packet with single data message from the current channel
    fn write_single_messages(
        packet: &mut Packet,
        messages: &mut VecDeque<SingleData>,
        num_messages: &mut usize,
        channel_id: ChannelId,
    ) -> Result<(), SerializationError> {
        packet.prewritten_size = packet
            .prewritten_size
            .checked_sub(varint_len(channel_id as u64) + 1)
            .ok_or(SerializationError::SubstractionOverflow)?;
        if *num_messages > 0 {
            trace!("Writing packet with {} messages", *num_messages);
            channel_id.to_bytes(&mut packet.payload)?;
            // write the number of messages for the current channel
            packet.payload.write_u8(*num_messages as u8)?;
            // write the messages
            for _ in 0..*num_messages {
                // TODO: deal with error
                let message = messages.pop_front().unwrap();
                message.to_bytes(&mut packet.payload)?;
                packet.prewritten_size = packet
                    .prewritten_size
                    .checked_sub(message.bytes_len())
                    .ok_or(SerializationError::SubstractionOverflow)?;
                // only send a MessageAck when the message has an id (otherwise we don't expect an ack)
                if let Some(id) = message.id {
                    packet.message_acks.push((
                        channel_id,
                        MessageAck {
                            message_id: id,
                            fragment_id: None,
                        },
                    ));
                }
            }
            *num_messages = 0;
        }
        Ok(())
    }

    // /// Uses multiple exponential searches to fill a packet. Has a good worst case runtime and doesn't
    // /// create any extraneous extension packets.
    // fn pack_multiple_exponential(mut messages: &[Message]) -> Vec<Packet> {
    //     /// A Vec<u8> prefixed by its length as a u32. Each [`Packet`] contains 1 or more [`Section`]s.
    //     struct Section(Vec<u8>);
    //     impl Section {
    //         fn len(&self) -> usize {
    //             self.0.len() + core::mem::size_of::<u32>()
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
    //                         packets.push(Packet(core::mem::take(&mut bytes)));
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

#[cfg(test)]
mod tests {
    use alloc::collections::VecDeque;

    use bevy::prelude::{default, TypePath};
    use bytes::Bytes;

    use lightyear_macros::ChannelInternal;

    use crate::channel::senders::fragment_sender::FragmentSender;
    use crate::packet::message::MessageId;
    use crate::prelude::*;

    use super::*;

    #[derive(ChannelInternal, TypePath)]
    struct Channel1;

    #[derive(ChannelInternal, TypePath)]
    struct Channel2;

    #[derive(ChannelInternal, TypePath)]
    struct Channel3;

    fn get_channel_registry() -> ChannelRegistry {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        };
        let mut c = ChannelRegistry::default();
        c.add_channel::<Channel1>(settings.clone());
        c.add_channel::<Channel2>(settings.clone());
        c.add_channel::<Channel3>(settings.clone());
        c
    }

    /// A bunch of small messages that all fit in the same packet
    #[test]
    fn test_pack_small_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![];
        let mut packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 1);
        let packet = packets.pop().unwrap();
        assert_eq!(packet.message_acks, vec![]);
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id1).unwrap(),
            &vec![small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![small_bytes.clone(), small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id3).unwrap(),
            &vec![small_bytes.clone()]
        );
        Ok(())
    }

    /// We cannot write the channel id of the next channel in the packet, so we need to finish the current
    /// packet and start a new one.
    /// We have 1200 -11 (header) -1 (channel_id) - 1(num_message) = 1184 bytes per message
    ///
    /// Test both with different channels and same channels
    #[test]
    fn test_pack_cannot_write_channel_id() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 1184]);
        let small_message = SingleData::new(None, small_bytes.clone());

        {
            let single_data = vec![(
                *channel_id1,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            )];
            let fragment_data = vec![];
            let packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
            assert_eq!(packets.len(), 2);
        }
        {
            let single_data = vec![
                (*channel_id1, VecDeque::from(vec![small_message.clone()])),
                (*channel_id2, VecDeque::from(vec![small_message.clone()])),
            ];
            let fragment_data = vec![];
            let packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
            assert_eq!(packets.len(), 2);
        }
        Ok(())
    }

    /// A bunch of small messages that all fit in the same packet
    #[test]
    fn test_pack_many_small_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (
                *channel_id1,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
            (
                *channel_id3,
                VecDeque::from(vec![small_message.clone(); 200]),
            ),
        ];
        let fragment_data = vec![];
        let packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 7);
        Ok(())
    }

    /// A bunch of small messages that fit in multiple packets
    #[test]
    fn test_pack_single_data_multiple_packets() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let small_bytes = Bytes::from(vec![7u8; 500]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![];
        let packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 2);
        Ok(())
    }

    /// Channel 1: small_message
    /// Channel 2: 2 small messages, 1 big fragment, 1 small fragment
    /// Channel 3: small message
    ///
    /// We should get 2 packets: 1 with the 1st big fragment, and 1 packet with the small fragment and all the small messages
    #[test]
    fn test_pack_big_messages() -> Result<(), PacketError> {
        let channel_registry = get_channel_registry();
        let mut manager = PacketBuilder::new(1.5);
        let channel_kind1 = ChannelKind::of::<Channel1>();
        let channel_id1 = channel_registry.get_net_from_kind(&channel_kind1).unwrap();
        let channel_kind2 = ChannelKind::of::<Channel2>();
        let channel_id2 = channel_registry.get_net_from_kind(&channel_kind2).unwrap();
        let channel_kind3 = ChannelKind::of::<Channel3>();
        let channel_id3 = channel_registry.get_net_from_kind(&channel_kind3).unwrap();

        let num_big_bytes = (1.5 * FRAGMENT_SIZE as f32) as usize;
        let big_bytes = Bytes::from(vec![1u8; num_big_bytes]);
        let fragmenter = FragmentSender::new();
        let fragments = fragmenter
            .build_fragments(MessageId(3), None, big_bytes.clone())?;

        let small_bytes = Bytes::from(vec![7u8; 10]);
        let small_message = SingleData::new(None, small_bytes.clone());

        let single_data = vec![
            (*channel_id1, VecDeque::from(vec![small_message.clone()])),
            (
                *channel_id2,
                VecDeque::from(vec![small_message.clone(), small_message.clone()]),
            ),
            (*channel_id3, VecDeque::from(vec![small_message.clone()])),
        ];
        let fragment_data = vec![(*channel_id2, fragments.clone().into())];
        let packets = manager.build_packets(Tick(0), single_data, fragment_data)?;
        assert_eq!(packets.len(), 2);

        let mut packets_queue: VecDeque<_> = packets.into();
        // 1st packet
        let packet = packets_queue.pop_front().unwrap();
        assert_eq!(
            packet.message_acks,
            vec![(
                *channel_id2,
                MessageAck {
                    message_id: MessageId(3),
                    fragment_id: Some(0),
                }
            )]
        );
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![fragments[0].bytes.clone()]
        );

        // 2nd packet
        let packet = packets_queue.pop_front().unwrap();
        assert_eq!(
            packet.message_acks,
            vec![(
                *channel_id2,
                MessageAck {
                    message_id: MessageId(3),
                    fragment_id: Some(1),
                }
            )]
        );
        let contents = packet.parse_packet_payload()?;
        assert_eq!(
            contents.get(channel_id1).unwrap(),
            &vec![small_bytes.clone()]
        );
        assert_eq!(
            contents.get(channel_id2).unwrap(),
            &vec![
                fragments[1].bytes.clone(),
                small_bytes.clone(),
                small_bytes.clone()
            ]
        );
        assert_eq!(
            contents.get(channel_id3).unwrap(),
            &vec![small_bytes.clone()]
        );
        Ok(())
    }

    // TODO: ADD MORE TESTS
}
