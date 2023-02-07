use std::{collections::HashMap, time::Duration};

use crate::shared::{
    serde::{BitWrite, BitWriter, Serde},
    ChannelId, ChannelMode, ChannelWriter, Channels, Message, PacketIndex, PacketNotifiable,
    ShortMessageIndex, Tick,
};

use super::channel_tick_buffer_sender::ChannelTickBufferSender;

pub struct TickBufferSender {
    channel_senders: HashMap<ChannelId, ChannelTickBufferSender>,
    #[allow(clippy::type_complexity)]
    packet_to_channel_map: HashMap<PacketIndex, Vec<(ChannelId, Vec<(Tick, ShortMessageIndex)>)>>,
}

impl TickBufferSender {
    pub fn new(tick_duration: &Duration) -> Self {
        // initialize senders
        let mut channel_senders = HashMap::new();
        for (channel_index, channel) in Channels::channels() {
            if let ChannelMode::TickBuffered(settings) = &channel.mode {
                channel_senders.insert(
                    *channel_index,
                    ChannelTickBufferSender::new(tick_duration, settings),
                );
            }
        }

        Self {
            channel_senders,
            packet_to_channel_map: HashMap::new(),
        }
    }

    // Outgoing Messages

    pub fn send_message(
        &mut self,
        host_tick: &Tick,
        channel_id: &ChannelId,
        message: Box<dyn Message>,
    ) {
        if let Some(channel) = self.channel_senders.get_mut(channel_id) {
            channel.send_message(host_tick, message);
        }
    }

    pub fn collect_outgoing_messages(
        &mut self,
        client_sending_tick: &Tick,
        server_receivable_tick: &Tick,
    ) {
        for channel in self.channel_senders.values_mut() {
            channel.collect_outgoing_messages(client_sending_tick, server_receivable_tick);
        }
    }

    pub fn has_outgoing_messages(&self) -> bool {
        for channel in self.channel_senders.values() {
            if channel.has_messages() {
                return true;
            }
        }
        false
    }

    pub fn write_messages(
        &mut self,
        channel_writer: &dyn ChannelWriter<Box<dyn Message>>,
        bit_writer: &mut BitWriter,
        packet_index: PacketIndex,
        host_tick: &Tick,
        has_written: &mut bool,
    ) {
        for (channel_index, channel) in &mut self.channel_senders {
            if !channel.has_messages() {
                continue;
            }

            // check that we can at least write a ChannelIndex and a MessageContinue bit
            let mut counter = bit_writer.counter();
            channel_index.ser(&mut counter);
            counter.write_bit(false);

            if counter.overflowed() {
                break;
            }

            // write ChannelContinue bit
            true.ser(bit_writer);

            // reserve MessageContinue bit
            bit_writer.reserve_bits(1);

            // write ChannelIndex
            channel_index.ser(bit_writer);

            // write Messages
            if let Some(message_ids) =
                channel.write_messages(channel_writer, bit_writer, host_tick, has_written)
            {
                self.packet_to_channel_map
                    .entry(packet_index)
                    .or_insert_with(Vec::new);
                let channel_list = self.packet_to_channel_map.get_mut(&packet_index).unwrap();
                channel_list.push((*channel_index, message_ids));
            }

            // write MessageContinue finish bit, release
            false.ser(bit_writer);
            bit_writer.release_bits(1);
        }
    }
}

impl PacketNotifiable for TickBufferSender {
    fn notify_packet_delivered(&mut self, packet_index: PacketIndex) {
        if let Some(channel_list) = self.packet_to_channel_map.get(&packet_index) {
            for (channel_index, message_ids) in channel_list {
                if let Some(channel) = self.channel_senders.get_mut(channel_index) {
                    for (tick, message_id) in message_ids {
                        channel.notify_message_delivered(tick, message_id);
                    }
                }
            }
        }
    }
}
