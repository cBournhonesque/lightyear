use std::collections::VecDeque;

use lightyear_serde::{BitReader, Error};

use crate::shared::{types::MessageIndex, wrapping_number::sequence_less_than};

use super::{
    indexed_message_reader::IndexedMessageReader,
    message_channel::{ChannelReader, ChannelReceiver},
};

// OrderedReliableReceiver

pub struct OrderedReliableReceiver<P> {
    oldest_received_message_id: MessageIndex,
    incoming_messages: VecDeque<(MessageIndex, Option<P>)>,
}

impl<P> Default for OrderedReliableReceiver<P> {
    fn default() -> Self {
        Self {
            oldest_received_message_id: 0,
            incoming_messages: VecDeque::default(),
        }
    }
}

impl<P> OrderedReliableReceiver<P> {
    pub fn buffer_message(&mut self, message_index: MessageIndex, message: P) {
        // moving from oldest incoming message to newest
        // compare existing slots and see if the message_id has been instantiated
        // already if it has, put the message into the slot
        // otherwise, keep track of what the last message id was
        // then add new empty slots at the end until getting to the incoming message id
        // then, once you're there, put the new message in

        if sequence_less_than(message_index, self.oldest_received_message_id) {
            // already moved sliding window past this message id
            return;
        }

        let mut index = 0;
        let mut found = false;

        loop {
            if index < self.incoming_messages.len() {
                if let Some((old_message_id, _)) = self.incoming_messages.get(index) {
                    if *old_message_id == message_index {
                        found = true;
                    }
                }

                if found {
                    let (_, old_message) = self.incoming_messages.get_mut(index).unwrap();
                    if old_message.is_none() {
                        *old_message = Some(message);
                    } else {
                        // already received this message
                    }
                    break;
                }
            } else {
                let next_message_id = self.oldest_received_message_id.wrapping_add(index as u16);

                if next_message_id == message_index {
                    self.incoming_messages
                        .push_back((next_message_id, Some(message)));
                    break;
                } else {
                    self.incoming_messages.push_back((next_message_id, None));
                }
            }

            index += 1;
        }
    }

    pub fn receive_messages(&mut self) -> Vec<P> {
        let mut output = Vec::new();
        loop {
            let mut has_message = false;
            if let Some((_, Some(_))) = self.incoming_messages.front() {
                has_message = true;
            }
            if has_message {
                let (_, message_opt) = self.incoming_messages.pop_front().unwrap();
                let message = message_opt.unwrap();
                output.push(message);
                self.oldest_received_message_id = self.oldest_received_message_id.wrapping_add(1);
            } else {
                break;
            }
        }
        output
    }
}

impl<P: Send + Sync> ChannelReceiver<P> for OrderedReliableReceiver<P> {
    fn read_messages(
        &mut self,
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<(), Error> {
        let id_w_msgs = IndexedMessageReader::read_messages(channel_reader, reader)?;
        for (id, message) in id_w_msgs {
            self.buffer_message(id, message);
        }
        Ok(())
    }

    fn receive_messages(&mut self) -> Vec<P> {
        self.receive_messages()
    }
}
