use crate::packet::wrapping_id::MessageId;
use crate::registry::message::{MessageKind, MessageRegistry};
use crate::registry::NetId;
use anyhow::Context;
use bitcode::encoding::Fixed;
use bitcode::read::Read;
use bitcode::write::Write;
use bitcode::{Decode, Encode};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct MessageContainer {
    pub(crate) id: Option<MessageId>,
    message: Box<dyn Message>,
}

impl MessageContainer {
    /// Serialize the message into a bytes buffer
    pub(crate) fn encode(
        &self,
        message_registry: &MessageRegistry,
        writer: &mut impl Write,
    ) -> anyhow::Result<()> {
        let net_id = message_registry
            .get_net_from_kind(&self.message.kind())
            .context("Could not find message kind")?;
        net_id.encode(Fixed, writer)?;
        self.id.encode(Fixed, writer)?;
        self.message.encode(writer)?;
        Ok(())
    }

    /// Deserialize from the bytes buffer into a Message
    pub(crate) fn decode(
        message_registry: &MessageRegistry,
        reader: &mut impl Read,
    ) -> anyhow::Result<Self> {
        let net_id = <NetId as Decode>::decode(Fixed, reader)?;
        let kind = message_registry
            .get_kind_from_net_id(net_id)
            .context("Could not find net id for message")?;
        let message_builder = message_registry
            .get_builder_from_kind(&kind)
            .context("Could not find message kind")?;
        message_builder.decode(message_registry, reader)
    }
}

pub trait MessageBuilder {
    /// Read bytes from the buffer and build a MessageContainer out of it
    ///
    /// This method is implemented automatically for all types that derive Message
    fn decode(
        &self,
        registry: &MessageRegistry,
        reader: &mut dyn Read,
    ) -> anyhow::Result<MessageContainer>;
}

pub trait Message: 'static {
    /// Get the MessageKind for the message
    fn kind(&self) -> MessageKind {
        MessageKind::of::<Self>()
    }
    fn get_builder() -> Box<dyn MessageBuilder>
    where
        Self: Sized;

    /// Encode a message into bytes
    fn encode(&self, buffer: &mut dyn Write) -> anyhow::Result<&[u8]>;
}

impl MessageContainer {
    // fn kind(&self) -> MessageKind {
    //     unimplemented!()
    // }

    pub fn new(message: Box<dyn Message>) -> Self {
        MessageContainer { id: None, message }
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
