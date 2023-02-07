use std::{collections::VecDeque, time::Duration};

use tracing::{info, warn};

use crate::shared::{
    sequence_greater_than, sequence_less_than,
    serde::{BitWrite, BitWriter, Serde, UnsignedVariableInteger},
    wrapping_diff, ChannelWriter, Instant, Message, ShortMessageIndex, Tick, TickBufferSettings,
    MESSAGE_HISTORY_SIZE,
};

pub struct ChannelTickBufferSender {
    sending_messages: OutgoingMessages,
    outgoing_messages: VecDeque<(Tick, Vec<(ShortMessageIndex, Box<dyn Message>)>)>,
    resend_interval: Duration,
    resend_interval_millis: u32,
    last_sent: Instant,
}

impl ChannelTickBufferSender {
    pub fn new(tick_duration: &Duration, settings: &TickBufferSettings) -> Self {
        let resend_interval = Duration::from_millis(
            ((settings.tick_resend_factor as u128) * tick_duration.as_millis()) as u64,
        );

        Self {
            sending_messages: OutgoingMessages::new(),
            outgoing_messages: VecDeque::new(),
            resend_interval,
            resend_interval_millis: resend_interval.as_millis() as u32,
            last_sent: Instant::now(),
        }
    }

    pub fn collect_outgoing_messages(
        &mut self,
        client_sending_tick: &Tick,
        server_receivable_tick: &Tick,
    ) {
        if self.last_sent.elapsed() >= self.resend_interval {
            // Remove messages that would never be able to reach the Server
            self.sending_messages
                .pop_back_until_excluding(server_receivable_tick);

            self.last_sent = Instant::now();

            // Loop through outstanding messages and add them to the outgoing list
            for (message_tick, message_map) in self.sending_messages.iter() {
                if sequence_greater_than(*message_tick, *client_sending_tick) {
                    //info!("found message that is more recent than client sending tick! (how?)");
                    break;
                }
                let messages = message_map.collect_messages();
                self.outgoing_messages.push_back((*message_tick, messages));
            }

            // if self.next_send_messages.len() > 0 {
            //     info!("next_send_messages.len() = {} messages",
            // self.next_send_messages.len()); }
        }
    }

    pub fn send_message(&mut self, host_tick: &Tick, message: Box<dyn Message>) {
        self.sending_messages.push(*host_tick, message);

        self.last_sent = Instant::now();
        self.last_sent.subtract_millis(self.resend_interval_millis);
    }

    pub fn has_messages(&self) -> bool {
        !self.outgoing_messages.is_empty()
    }

    // Tick Buffer Message Writing

    pub fn write_messages(
        &mut self,
        channel_writer: &dyn ChannelWriter<Box<dyn Message>>,
        bit_writer: &mut BitWriter,
        host_tick: &Tick,
        has_written: &mut bool,
    ) -> Option<Vec<(Tick, ShortMessageIndex)>> {
        let mut last_written_tick = *host_tick;
        let mut output = Vec::new();

        loop {
            if self.outgoing_messages.is_empty() {
                break;
            }

            let (message_tick, messages) = self.outgoing_messages.front().unwrap();

            // check that we can write the next message
            let mut counter = bit_writer.counter();
            self.write_message(
                channel_writer,
                &mut counter,
                &last_written_tick,
                message_tick,
                messages,
            );

            if counter.overflowed() {
                // if nothing useful has been written in this packet yet,
                // send warning about size of message being too big
                if !*has_written {
                    self.warn_overflow(messages, counter.bits_needed(), bit_writer.bits_free());
                }

                break;
            }

            *has_written = true;

            // write MessageContinue bit
            true.ser(bit_writer);

            // write data
            let message_ids = self.write_message(
                channel_writer,
                bit_writer,
                &last_written_tick,
                &message_tick,
                &messages,
            );
            last_written_tick = *message_tick;
            for message_id in message_ids {
                output.push((*message_tick, message_id));
            }

            // pop message we've written
            self.outgoing_messages.pop_front();
        }
        Some(output)
    }

    /// Writes a Command into the Writer's internal buffer, which will
    /// eventually be put into the outgoing packet
    fn write_message(
        &self,
        channel_writer: &dyn ChannelWriter<Box<dyn Message>>,
        bit_writer: &mut dyn BitWrite,
        last_written_tick: &Tick,
        message_tick: &Tick,
        messages: &Vec<(ShortMessageIndex, Box<dyn Message>)>,
    ) -> Vec<ShortMessageIndex> {
        let mut message_ids = Vec::new();

        // write message tick diff
        // this is reversed (diff is always negative, but it's encoded as positive)
        // because packet tick is always larger than past ticks
        let message_tick_diff = wrapping_diff(*message_tick, *last_written_tick);
        let message_tick_diff_encoded = UnsignedVariableInteger::<3>::new(message_tick_diff);
        message_tick_diff_encoded.ser(bit_writer);

        // write number of messages
        let message_count = UnsignedVariableInteger::<3>::new(messages.len() as u64);
        message_count.ser(bit_writer);

        let mut last_id_written: ShortMessageIndex = 0;
        for (message_id, message) in messages {
            // write message id diff
            let id_diff = UnsignedVariableInteger::<2>::new(*message_id - last_id_written);
            id_diff.ser(bit_writer);

            // write payload
            channel_writer.write(bit_writer, message);

            // record id for output
            message_ids.push(*message_id);
            last_id_written = *message_id;
        }

        message_ids
    }

    pub fn notify_message_delivered(&mut self, tick: &Tick, message_id: &ShortMessageIndex) {
        self.sending_messages.remove_message(tick, message_id);
    }

    fn warn_overflow(
        &self,
        messages: &Vec<(ShortMessageIndex, Box<dyn Message>)>,
        bits_needed: u16,
        bits_free: u16,
    ) {
        let mut message_names = "".to_string();
        let mut added = false;
        for (_id, message) in messages {
            if added {
                message_names.push(',');
            } else {
                added = true;
            }
            message_names.push_str(&message.name());
        }
        panic!(
            "Packet Write Error: Blocking overflow detected! Messages of type `{message_names}` requires {bits_needed} bits, but packet only has {bits_free} bits available! This condition should never be reached, as large Messages should be Fragmented in the Reliable channel"
        )
    }
}

// MessageMap
struct MessageMap {
    list: Vec<Option<Box<dyn Message>>>,
}

impl MessageMap {
    pub fn new() -> Self {
        MessageMap { list: Vec::new() }
    }

    pub fn insert(&mut self, message: Box<dyn Message>) {
        self.list.push(Some(message));
    }

    pub fn collect_messages(&self) -> Vec<(ShortMessageIndex, Box<dyn Message>)> {
        let mut output = Vec::new();
        for (index, message_opt) in self.list.iter().enumerate() {
            if let Some(message) = message_opt {
                output.push((index as u8, message.clone()));
            }
        }
        output
    }

    pub fn remove(&mut self, message_id: &ShortMessageIndex) {
        if let Some(container) = self.list.get_mut(*message_id as usize) {
            *container = None;
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

// OutgoingMessages

struct OutgoingMessages {
    // front big, back small
    // front recent, back past
    buffer: VecDeque<(Tick, MessageMap)>,
}

impl OutgoingMessages {
    pub fn new() -> Self {
        OutgoingMessages {
            buffer: VecDeque::new(),
        }
    }

    // should only push increasing ticks of messages
    pub fn push(&mut self, message_tick: Tick, message_protocol: Box<dyn Message>) {
        if let Some((front_tick, msg_map)) = self.buffer.front_mut() {
            if message_tick == *front_tick {
                // been here before, cool
                msg_map.insert(message_protocol);
                return;
            }

            if sequence_less_than(message_tick, *front_tick) {
                warn!("This method should always receive increasing or equal Ticks! \
                Received Tick: {message_tick} after receiving {front_tick}. \
                Possibly try ensuring that Client.send_message() is only called on this channel once per Tick?");
                return;
            }
        } else {
            // nothing is in here
        }

        let mut msg_map = MessageMap::new();
        msg_map.insert(message_protocol);
        self.buffer.push_front((message_tick, msg_map));

        // a good time to prune down this list
        while self.buffer.len() > MESSAGE_HISTORY_SIZE.into() {
            self.buffer.pop_back();
            info!("pruning outgoing_messages buffer cause it got too big");
        }
    }

    pub fn pop_back_until_excluding(&mut self, until_tick: &Tick) {
        loop {
            if let Some((old_tick, _)) = self.buffer.back() {
                if sequence_less_than(*until_tick, *old_tick) {
                    return;
                }
            } else {
                return;
            }

            self.buffer.pop_back();
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Tick, MessageMap)> {
        self.buffer.iter()
    }

    pub fn remove_message(&mut self, tick: &Tick, message_id: &ShortMessageIndex) {
        let mut index = self.buffer.len();

        if index == 0 {
            // empty condition
            return;
        }

        loop {
            index -= 1;

            let mut remove = false;

            if let Some((old_tick, message_map)) = self.buffer.get_mut(index) {
                if *old_tick == *tick {
                    // found it!
                    message_map.remove(message_id);
                    //info!("removed delivered message! tick: {}, msg_id: {}", tick, msg_id);
                    if message_map.len() == 0 {
                        remove = true;
                    }
                } else {
                    // if tick is less than old tick, no sense continuing, only going to get bigger
                    // as we go
                    if sequence_greater_than(*old_tick, *tick) {
                        return;
                    }
                }
            }

            if remove {
                self.buffer.remove(index);
            }

            if index == 0 {
                return;
            }
        }
    }
}
