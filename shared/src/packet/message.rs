use crate::packet::wrapping_id::MessageId;
use bitcode::__private::{Encoding, Write};
use bitcode::{Decode, Encode, Error};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};


// TODO: adopt Message trait and MessageContainer from naia

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Message {
    pub(crate) id: Option<MessageId>,
    // kind
    data: Bytes,
}

impl Encode for Message {
    const ENCODE_MIN: usize = 1;
    const ENCODE_MAX: usize = usize::MAX;

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<(), Error> {
        self.id.encode(encoding, writer)?;
        self.data.as_ref().encode(encoding, writer)?;
        Ok(())
    }
}

impl Decode for Message {
    const DECODE_MIN: usize = 1;
    const DECODE_MAX: usize = usize::MAX;

    fn decode(
        encoding: impl Encoding,
        reader: &mut impl bitcode::__private::Read,
    ) -> Result<Message, Error> {
        let id = Option::<MessageId>::decode(encoding, reader)?;
        let data = Vec::<u8>::decode(encoding, reader)?;
        Ok(Message {
            id,
            data: Bytes::from(data),
        })
    }
}

impl Message {
    // fn kind(&self) -> MessageKind {
    //     unimplemented!()
    // }

    pub fn new(data: Bytes) -> Self {
        Message { id: None, data }
    }

    pub fn set_id(&mut self, id: MessageId) {
        self.id = Some(id);
    }

    /// Bit length of the serialized message (including the message id and message kind)
    pub fn bit_len(&self) -> u32 {
        let mut len = 0;
        if let Some(_) = self.id {
            len += 2;
        }
        len += self.data.len() as u32;
        len
    }

    // TODO: right now it means each message has byte-padding
    /// Serialize the message into a bytes buffer
    pub(crate) fn to_bytes(&self) -> anyhow::Result<Bytes> {
        // TODO: optimize the extra 2 bytes?
        let mut bytes = BytesMut::with_capacity(self.data.len() + 2);
        if let Some(id) = self.id {
            let mut buffer = bitcode::Buffer::with_capacity(2);
            let id_bytes = buffer.encode(&id)?;
            bytes.extend(id_bytes);
        }
        // TODO: this does a copy?
        bytes.extend(self.data.iter());
        Ok(bytes.freeze())
    }
}
