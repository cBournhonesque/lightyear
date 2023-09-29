//! Module to define how messages are stored into packets

use crate::packet::manager::PacketManager;
use crate::packet::message::MessageContainer;
use crate::packet::packet::Packet;
use crate::packet::wrapping_id::MessageId;
use std::collections::VecDeque;

pub struct MessagePacker;

trait MessageIterator {
    fn is_empty(&self) -> bool;
    fn front(&self) -> Option<&(Option<MessageId>, MessageContainer)>;
    fn pop_front(&mut self) -> Option<(Option<MessageId>, MessageContainer)>;
}

impl MessagePacker {
    /// Pack messages into packets
    /// Return the remaining list of messages to send
    pub fn pack_messages(
        mut messages_to_send: VecDeque<MessageContainer>,
        packet_manager: &mut PacketManager,
    ) -> (Vec<Packet>, VecDeque<MessageContainer>) {
        let mut packets = Vec::new();
        // build new packet
        'packet: loop {
            let mut packet = packet_manager.build_new_packet();

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
                if packet_manager
                    .can_add_message(&mut packet, &message)
                    .is_ok_and(|b| b)
                {
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
