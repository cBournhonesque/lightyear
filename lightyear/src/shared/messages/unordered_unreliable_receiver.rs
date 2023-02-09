use std::{collections::VecDeque, mem};

use lightyear_serde::{BitReader, Serde, Error};

use super::message_channel::{ChannelReader, ChannelReceiver};

pub struct UnorderedUnreliableReceiver<P> {
    incoming_messages: VecDeque<P>,
}

impl<P> UnorderedUnreliableReceiver<P> {
    pub fn new() -> Self {
        Self {
            incoming_messages: VecDeque::new(),
        }
    }

    fn read_message(
        &mut self,
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<P, Error> {
        // read payload
        channel_reader.read(reader)
    }

    fn recv_message(&mut self, message: P) {
        self.incoming_messages.push_back(message);
    }
}

impl<P: Send + Sync> ChannelReceiver<P> for UnorderedUnreliableReceiver<P> {
    fn read_messages(
        &mut self,
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<(), Error> {
        loop {
            let channel_continue = bool::de(reader)?;
            if !channel_continue {
                break;
            }

            let message = self.read_message(channel_reader, reader)?;
            self.recv_message(message);
        }

        Ok(())
    }

    fn receive_messages(&mut self) -> Vec<P> {
        Vec::from(mem::take(&mut self.incoming_messages))
    }
}
