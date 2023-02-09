use std::marker::PhantomData;

use lightyear_serde::{BitReader, Serde, Error, UnsignedVariableInteger};

use crate::shared::{messages::message_channel::ChannelReader, types::MessageIndex};

/// Building block for the message channels. Read messages that have a MessageIndex attached to them
pub struct IndexedMessageReader<P> {
    phantom_p: PhantomData<P>,
}

impl<P> IndexedMessageReader<P> {
    pub fn read_messages(
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
    ) -> Result<Vec<(MessageIndex, P)>, Error> {
        let mut last_read_id: Option<MessageIndex> = None;
        let mut output = Vec::new();

        loop {
            let channel_continue = bool::de(reader)?;
            if !channel_continue {
                break;
            }

            let id_w_msg = Self::read_message(channel_reader, reader, &last_read_id)?;
            last_read_id = Some(id_w_msg.0);
            output.push(id_w_msg);
        }

        Ok(output)
    }

    fn read_message(
        channel_reader: &dyn ChannelReader<P>,
        reader: &mut BitReader,
        last_read_id: &Option<MessageIndex>,
    ) -> Result<(MessageIndex, P), Error> {
        let message_id: MessageIndex = if let Some(last_id) = last_read_id {
            let id_diff = UnsignedVariableInteger::<3>::de(reader)?.get() as MessageIndex;
            last_id.wrapping_add(id_diff)
        } else {
            // read message id
            MessageIndex::de(reader)?
        };

        // read payload
        let new_message = channel_reader.read(reader)?;

        Ok((message_id, new_message))
    }
}
