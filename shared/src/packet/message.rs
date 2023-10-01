use std::fmt::Debug;

use crate::MessageProtocol;
use anyhow::Context;
use bitcode::write::Write;
use dyn_clone::DynClone;
use serde::Serialize;

use crate::packet::wrapping_id::MessageId;
use crate::registry::message::MessageKind;
use crate::registry::NetId;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::serialize::writer::WriteBuffer;

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
// #[derive(Serialize, Deserialize)]
pub struct MessageContainer<P: MessageProtocol> {
    pub(crate) id: Option<MessageId>,
    message: P,
}

impl<P: MessageProtocol> MessageContainer<P> {
    /// Serialize the message into a bytes buffer
    /// Returns the number of bits written
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.serialize(&self.id)?;
        writer.serialize(&self.message)?;
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }

    /// Deserialize from the bytes buffer into a Message
    pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self> {
        let id = reader.deserialize::<Option<MessageId>>()?;
        let message = reader.deserialize::<P>()?;
        Ok(Self { id, message })
    }
    fn kind(&self) -> MessageKind {
        unimplemented!()
    }

    pub fn new(message: Box<dyn Message>) -> Self {
        MessageContainer { id: None, message }
    }

    pub fn set_id(&mut self, id: MessageId) {
        self.id = Some(id);
    }
}

// pub trait MessageBuilder {
//     /// Read bytes from the buffer and build a MessageContainer out of it
//     ///
//     /// This method is implemented automatically for all types that derive Message
//     fn decode(
//         &self,
//         registry: &MessageRegistry,
//         reader: &mut impl ReadBuffer,
//     ) -> anyhow::Result<MessageContainer>;
// }

pub trait Message: 'static {
    /// Get the MessageKind for the message
    fn kind(&self) -> MessageKind;

    // fn get_builder() -> Box<dyn MessageBuilder>
    // where
    //     Self: Sized;
    //
    // /// Encode a message into bytes
    // fn encode(&self, buffer: &mut dyn Write) -> anyhow::Result<&[u8]>;
}

// dyn_clone::clone_trait_object!(Message);
