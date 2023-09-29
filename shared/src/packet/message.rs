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
use std::any::TypeId;
use std::fmt::Debug;

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
pub struct MessageContainer {
    pub(crate) id: Option<MessageId>,
    message: Box<dyn Message>,
}

impl MessageContainer {
    /// Serialize the message into a bytes buffer
    /// Returns the number of bits written
    pub(crate) fn encode(
        &self,
        message_registry: &MessageRegistry,
        writer: &mut impl Write,
    ) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        let net_id = message_registry
            .get_net_from_kind(&self.message.kind())
            .context("Could not find message kind")?;
        net_id.encode(Fixed, writer)?;
        self.id.encode(Fixed, writer)?;
        self.message.encode(writer)?;
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
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
    fn kind(&self) -> MessageKind;

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
}
