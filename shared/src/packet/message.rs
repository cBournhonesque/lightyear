use bevy::prelude::Event;
use bitcode::encoding::{Fixed, Gamma};
use bitcode::{Decode, Encode};
use bytes::{Bytes, BytesMut};
use std::fmt::Debug;

use crate::connection::events::EventContext;
use crate::packet::packet::{FragmentedPacket, FRAGMENT_SIZE};
use serde::Serialize;

use crate::protocol::BitSerializable;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::utils::named::Named;
use crate::utils::wrapping_id;

// strategies to avoid copying:
// - have a net_id for each message or component
//   be able to take a reference to a M and serialize it into bytes (so we can cheaply clone)
//   in the serialized message, include the net_id (for decoding)
//   no bit padding

/// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);

pub type FragmentIndex = u8;

/// Struct to keep track of which messages/slices have been received by the remote
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub(crate) struct MessageAck {
    pub(crate) message_id: MessageId,
    pub(crate) fragment_id: Option<FragmentIndex>,
}

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

#[derive(Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
/// This structure contains the bytes for a single 'logical' message
///
/// We store the bytes instead of the message directly.
/// This lets us serialize the message very early and then pass it around with cheap clones
/// The message/component does not need to implement Clone anymore!
/// Also we know the size of the message early, which is useful for fragmentation.
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
        writer.encode(&self.id, Fixed)?;
        // Maybe we should just newtype Bytes so we could implement encode for it separately?

        // we encode Bytes by writing the length first
        // writer.encode(&(self.bytes.len() )?;
        // writer.encode(&self.bytes.to_vec(), Fixed)?;
        writer.encode(self.bytes.as_ref(), Fixed)?;
        // writer.serialize(self.bytes.as_ref())?;
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }

    // TODO: are we doing an extra copy here?
    pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self> {
        let id = reader.decode::<Option<MessageId>>(Fixed)?;

        // the encoding wrote the length as usize with gamma encoding
        // let num_bytes = reader.decode::<usize>(Gamma)?;
        // let num_bytes_non_zero = std::num::NonZeroUsize::new(num_bytes)
        //     .ok_or_else(|| anyhow::anyhow!("num_bytes is 0"))?;
        // let read_bytes = reader.read_bytes(num_bytes_non_zero)?;
        // let bytes = BytesMut::from(read_bytes).freeze();

        // let read_bytes =
        // TODO: ANNOYING; AVOID ALLOCATING HERE
        //  - we already do a copy from the network io into the ReadBuffer
        //  - then we do an allocation here to read the Bytes into SingleData
        //  - (we do a copy when composing the fragment from bytes)
        //  - we do an extra copy when allocating a new ReadBuffer from the SingleData Bytes

        // TODO: use ReadBuffer/WriteBuffer only when decoding/encoding messages the first time/final time!
        //  once we have the Bytes object, manually do memcpy, aligns the final bytes manually!
        //  (encode header, channel ids, message ids, etc. myself)
        //  maybe use BitVec to construct the final objects?

        // TODO: DO A FLOW OF THE DATA/COPIES/CLONES DURING ENCODE/DECODE TO DECIDE
        //  HOW THE LIFETIMES FLOW!
        // let read_bytes = <Vec<u8> as ReadBuffer>::decode(reader, Fixed)?;
        let read_bytes = reader.decode::<Vec<u8>>(Fixed)?;

        //
        // let bytes = reader.decode::<&[u8]>()?;
        Ok(Self {
            id,
            bytes: Bytes::from(read_bytes),
            // bytes: Bytes::copy_from_slice(read_bytes),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentData {
    // we always need a message_id for fragment messages, for re-assembly
    pub message_id: MessageId,
    pub fragment_id: FragmentIndex,
    pub num_fragments: FragmentIndex,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
}

impl FragmentData {
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.encode(&self.message_id, Fixed)?;
        writer.encode(&self.fragment_id, Gamma)?;
        writer.encode(&self.num_fragments, Gamma)?;
        // TODO: be able to just concat the bytes to the buffer?
        if self.is_last_fragment() {
            /// writing the slice includes writing the length of the slice
            writer.encode(self.bytes.as_ref(), Fixed);
            // writer.serialize(&self.bytes.to_vec());
            // writer.serialize(&self.fragment_message_bytes.as_ref());
        } else {
            let bytes_array: [u8; FRAGMENT_SIZE] = self.bytes.as_ref().try_into().unwrap();
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            writer.encode(&bytes_array, Fixed);
        }
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }

    pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let message_id = reader.decode::<MessageId>(Fixed)?;
        let fragment_id = reader.decode::<FragmentIndex>(Gamma)?;
        let num_fragments = reader.decode::<FragmentIndex>(Gamma)?;
        let mut bytes: Bytes;
        if fragment_id == num_fragments - 1 {
            // let num_bytes = reader.decode::<usize>(Gamma)?;
            // let num_bytes_non_zero = std::num::NonZeroUsize::new(num_bytes)
            //     .ok_or_else(|| anyhow::anyhow!("num_bytes is 0"))?;
            // let read_bytes = reader.read_bytes(num_bytes_non_zero)?;
            // reader.
            // let read_bytes = reader.decode::<&[u8]>()?;
            // TODO: avoid the extra copy
            //  - maybe have the encoding of bytes be
            let read_bytes = reader.decode::<Vec<u8>>(Fixed)?;
            bytes = Bytes::from(read_bytes);
        } else {
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            let read_bytes = reader.decode::<[u8; FRAGMENT_SIZE]>(Fixed)?;
            // TODO: avoid the extra copy
            let bytes_vec: Vec<u8> = read_bytes.to_vec();
            bytes = Bytes::from(bytes_vec);
        }
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            bytes,
        })
    }

    pub(crate) fn is_last_fragment(&self) -> bool {
        self.fragment_id == self.num_fragments - 1
    }

    fn num_fragment_bytes(&self) -> usize {
        if self.is_last_fragment() {
            self.bytes.len()
        } else {
            FRAGMENT_SIZE
        }
    }
}
impl MessageContainer {
    pub(crate) fn message_id(&self) -> Option<MessageId> {
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
        match self {
            MessageContainer::Single(data) => data.id = Some(id),
            MessageContainer::Fragment(data) => data.message_id = id,
        };
    }

    /// Get access to the underlying bytes (clone is a cheap operation for `Bytes`)
    pub fn bytes(&self) -> Bytes {
        match self {
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
pub trait Message: EventContext + Named {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReadWordBuffer, WriteWordBuffer};
    use bitvec::prelude::AsBits;

    #[test]
    fn test_serde_single_data() {
        let data = SingleData::new(Some(MessageId(1)), vec![9, 3].into());
        let mut writer = WriteWordBuffer::with_capacity(10);
        let a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes.as_ref());
        let decoded = SingleData::decode(&mut reader).unwrap();

        // dbg!(bitvec::vec::BitVec::<u8>::from_slice(&bytes));
        dbg!(&bytes);
        // dbg!(&writer.num_bits_written());
        // dbg!(&decoded.id);
        // dbg!(&decoded.bytes.as_ref());
        assert_eq!(decoded, data);
        dbg!(&writer.num_bits_written());
        // assert_eq!(writer.num_bits_written(), 5 * u8::BITS as usize);
    }

    #[test]
    fn test_serde_fragment_data() {
        let bytes = Bytes::from(vec![0; 10]);
        let data = FragmentData {
            message_id: MessageId(0),
            fragment_id: 2,
            num_fragments: 3,
            bytes: bytes.clone(),
        };
        let mut writer = WriteWordBuffer::with_capacity(10);
        let a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes.as_ref());
        let decoded = FragmentData::decode(&mut reader).unwrap();

        // dbg!(bitvec::vec::BitVec::<u8>::from_slice(&bytes));
        dbg!(&bytes);
        // dbg!(&writer.num_bits_written());
        // dbg!(&decoded.id);
        // dbg!(&decoded.bytes.as_ref());
        assert_eq!(decoded, data);
        dbg!(&writer.num_bits_written());
        // assert_eq!(writer.num_bits_written(), 5 * u8::BITS as usize);
    }
}
