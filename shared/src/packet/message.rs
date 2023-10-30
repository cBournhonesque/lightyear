use bevy::prelude::Event;
use bitcode::{Decode, Encode};
use bytes::Bytes;
use std::fmt::Debug;

use crate::connection::events::EventContext;
use crate::packet::packet::{FragmentData, FragmentedPacket};
use serde::Serialize;

use crate::packet::wrapping_id::MessageId;
use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;

// strategies to avoid copying:
// - have a net_id for each message or component
//   be able to take a reference to a M and serialize it into bytes (so we can cheaply clone)
//   in the serialized message, include the net_id (for decoding)
//   no bit padding

// -

/// A Message is a logical unit of data that should be transmitted over a network
///
/// The message can be small (multiple messages can be sent in a single packet)
/// or big (a single message can be split between multiple packets)
///
/// A Message knows how to serialize itself (messageType + Data)
/// and knows how many bits it takes to serialize itself
///
/// In the message container, we already store the serialized representation of the message.
/// The main reason is so that we can avoid copies, by directly serializing references into raw bits
// #[derive(Serialize, Deserialize)]
// #[derive(Clone, PartialEq, Debug)]
// pub struct MessageContainer<P: BitSerializable> {
//     pub(crate) id: Option<MessageId>,
//     // we use bytes so we can cheaply copy the message container (for example in reliable sender)
//     pub(crate) message: Bytes,
//     // TODO: we use num_bits to avoid padding each message to a full byte when serializing
//     // num_bits: usize,
//     // message: P,
//     marker: PhantomData<P>,
// }
//
// impl<P: BitSerializable> MessageContainer<P> {
//     /// Serialize the message into a bytes buffer
//     /// Returns the number of bits written
//     pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
//         let num_bits_before = writer.num_bits_written();
//         writer.serialize(&self.id)?;
//         // TODO: only serialize the bits that matter (without padding!)
//         self.message.as_ref().encode(writer)?;
//         let num_bits_written = writer.num_bits_written() - num_bits_before;
//         Ok(num_bits_written)
//     }
//
//     /// Deserialize from the bytes buffer into a Message
//     pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self> {
//         let id = reader.deserialize::<Option<MessageId>>()?;
//         let raw_bytes = <[u8]>::decode(reader)?;
//         let message = Bytes::from(raw_bytes);
//         Ok(Self {
//             id,
//             message,
//             marker: Default::default(),
//         })
//     }
//
//     pub fn new<P: BitSerializable>(message: &P) -> Self {
//         // TODO: reuse the same buffer for writing message containers
//         let mut buffer = WriteWordBuffer::with_capacity(1024);
//         buffer.start_write();
//         message.encode(&mut buffer).unwrap();
//         let bytes = buffer.finish_write();
//         // let num_bits_written = buffer.num_bits_written();
//         MessageContainer {
//             id: None,
//             message: Bytes::from(bytes),
//             marker: Default::default(),
//         }
//     }
//
//     pub fn set_id(&mut self, id: MessageId) {
//         self.id = Some(id);
//     }
//
//     pub fn inner(self) -> P {
//         // TODO: have a way to do this without any copy?
//         let mut reader = ReadWordBuffer::start_read(self.message.as_ref());
//         P::decode(&mut reader).unwrap()
//     }
// }

// TODO: we could just store Bytes in MessageContainer to serialize very early
//  important thing is to re-use the Writer to allocate only once?
//  pros: we might not require messages/components to be clone anymore if we serialize them very early!
//  also we know the size of the message early, which is useful for fragmentation
// #[derive(Clone, PartialEq, Debug)]
// pub struct MessageContainer<P> {
//     pub(crate) id: Option<MessageId>,
//     message: P,
// }

pub enum MessageContainer {
    Single(SingleData),
    Fragment(FragmentData),
}

impl From<FragmentData> for MessageContainer {
    fn from(value: FragmentData) -> Self {
        Self::Fragment(value)
    }
}

impl From<SingleData> for MessageContainer {
    fn from(value: SingleData) -> Self {
        Self::Single(value)
    }
}

#[derive(Encode, Decode, Clone, Debug)]
pub struct SingleData {
    pub id: Option<MessageId>,
    pub bytes: Bytes,
}

impl SingleData {
    pub fn new(id: Option<MessageId>, bytes: Bytes) -> Self {
        Self { id, bytes }
    }

    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.serialize(&self.id)?;
        writer.serialize(self.bytes.as_ref())?;
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }
}

impl MessageContainer {
    pub(crate) fn id(&self) -> Option<MessageId> {
        match &self {
            MessageContainer::Single(data) => data.id,
            MessageContainer::Fragment(data) => Some(data.message_id),
        }
    }

    pub(crate) fn is_fragment(&self) -> bool {
        match &self {
            MessageContainer::Single(_) => false,
            MessageContainer::Fragment(_) => true,
        }
    }

    /// Serialize the message into a bytes buffer
    /// Returns the number of bits written
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        match &self {
            MessageContainer::Single(data) => data.encode(writer),
            MessageContainer::Fragment(data) => data.encode(writer),
        }
    }

    // TODO: here we could do decode<M: BitSerializable> to return a MessageContainer<M>
    // TODO: add decode_single and decode_slice (packet manager knows which type to decode)

    /// Deserialize from the bytes buffer into a Message
    // pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self> {
    //     let id = reader.deserialize::<Option<MessageId>>()?;
    //     let message = P::decode(reader)?;
    //     Ok(Self { id, message })
    // }
    // fn kind(&self) -> MessageKind {
    //     unimplemented!()
    // }

    // pub fn new(message: P) -> Self {
    //     MessageContainer { id: None, message }
    // }

    pub fn set_id(&mut self, id: MessageId) {
        match &mut self {
            MessageContainer::Single(data) => data.id = Some(id),
            MessageContainer::Fragment(data) => data.message_id = id,
        };
    }

    /// Get access to the underlying bytes (clone is a cheap operation for `Bytes`)
    pub fn bytes(&self) -> Bytes {
        match &self {
            MessageContainer::Single(data) => data.bytes.clone(),
            MessageContainer::Fragment(data) => data.bytes.clone(),
        }
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

// TODO: for now messages must be able to be used as events, since we output them in our message events
pub trait Message: EventContext {
    // /// Get the MessageKind for the message
    // fn kind(&self) -> MessageKind;

    // fn get_builder() -> Box<dyn MessageBuilder>
    // where
    //     Self: Sized;
    //
    // /// Encode a message into bytes
    // fn encode(&self, buffer: &mut dyn Write) -> anyhow::Result<&[u8]>;
}

// dyn_clone::clone_trait_object!(Message);
