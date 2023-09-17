//! Module to define how messages are stored into packets

use crate::packet::message::Message;
use crate::packet::packet::{Packet, PacketWriter, SinglePacket};
use crate::packet::wrapping_id::MessageId;
use std::collections::VecDeque;

pub struct MessagePacker;

trait MessageIterator {
    fn is_empty(&self) -> bool;
    fn front(&self) -> Option<&(Option<MessageId>, Message)>;
    fn pop_front(&mut self) -> Option<(Option<MessageId>, Message)>;
}

impl MessagePacker {
    pub fn pack_messages(
        // TODO: use an iterator of Messages (from most urgent to least urgent)
        // messages_to_send: &mut VecDeque<(MessageId, Message)>,
        // /// Iterator of messages from most urgent to least urgent
        // messages_to_send: &mut impl MessageIterator,
        messages_to_send: &mut VecDeque<(Option<MessageId>, Message)>,
        packet_writer: &mut PacketWriter,
    ) -> Vec<Packet> {
        let mut packets = Vec::new();
        // build new packet
        'packet: loop {
            let mut packet = packet_writer.build_new_packet();

            // add messages to packet
            'message: loop {
                // TODO: check if message size is too big for a single packet, in which case we fragment!

                if messages_to_send.is_empty() {
                    break 'packet;
                }
                let (message_id, message) = messages_to_send.front().unwrap();
                // TODO: AVOID THIS CLONE!
                match packet_writer.try_add_message(
                    &mut packet,
                    message_id.unwrap(),
                    message.clone(),
                ) {
                    Ok(_) => {
                        // message was added to packet, remove from messages_to_send
                        messages_to_send.pop_front();
                    }
                    Err(_) => {
                        // message was not added to packet, packet is full
                        packets.push(packet);
                        break 'message;
                    }
                }
            }
        }
        packets
    }
}
