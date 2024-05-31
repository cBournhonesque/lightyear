use std::fmt::Debug;

use bevy::ecs::entity::MapEntities;
use bytes::{BufMut, Bytes};
use octets::OctetsMut;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use bitcode::encoding::{Fixed, Gamma};

use crate::packet::packet::FRAGMENT_SIZE;
use crate::protocol::{BitSerializable, EventContext};
use crate::serialize::octets::{SerializationError, ToBytes};
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::tick_manager::Tick;
use crate::utils::wrapping_id::wrapping_id;

// strategies to avoid copying:
// - have a net_id for each message or component
//   be able to take a reference to a M and serialize it into bytes (so we can cheaply clone)
//   in the serialized message, include the net_id (for decoding)
//   no bit padding

// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);

// TODO: for now messages must be able to be used as events, since we output them in our message events
/// A [`Message`] is basically any type that can be (de)serialized over the network.
///
/// Every type that can be sent over the network must implement this trait.
///
pub trait Message: EventContext + BitSerializable + DeserializeOwned + Serialize {}
impl<T: EventContext + BitSerializable + DeserializeOwned + Serialize> Message for T {}

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
// TODO: we could just store Bytes in MessageContainer to serialize very early
//  important thing is to re-use the Writer to allocate only once?
//  pros: we might not require messages/components to be clone anymore if we serialize them very early!
//  also we know the size of the message early, which is useful for fragmentation

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// This structure contains the bytes for a single 'logical' message
///
/// We store the bytes instead of the message directly.
/// This lets us serialize the message very early and then pass it around with cheap clones
/// The message/component does not need to implement Clone anymore!
/// Also we know the size of the message early, which is useful for fragmentation.
pub struct SingleData {
    // TODO: MessageId is from 1 to 65535, so that we can use 0 to represent None?
    pub id: Option<MessageId>,
    pub bytes: Bytes,
    // we do not encode the priority in the packet
    #[serde(skip)]
    pub priority: f32,
}

impl ToBytes for SingleData {
    // TODO: how to avoid the option taking 1 byte?
    fn len(&self) -> usize {
        octets::varint_len(self.bytes.len() as u64) + self.bytes.len() + self.id.map_or(1, |_| 3)
    }

    fn to_bytes(&self, octets: &mut OctetsMut) -> Result<(), SerializationError> {
        if let Some(id) = self.id {
            octets.put_u8(1)?;
            octets.put_u16(id.0)?;
        } else {
            octets.put_u8(1)?;
        }
        octets.put_u16(self.id.map_or(0, |id| id.0))?;
        octets.put_varint(self.bytes.len() as u64)?;
        octets.put_bytes(self.bytes.as_ref())?;
        Ok(())
    }

    fn from_bytes(octets: &mut octets::Octets) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let id = if octets.get_u8()? == 1 {
            Some(MessageId(octets.get_u16()?))
        } else {
            None
        };
        let len = octets.get_varint()? as usize;
        let bytes = Bytes::from(octets.get_bytes(len)?);
        Ok(Self {
            id,
            bytes,
            priority: 1.0,
        })
    }
}

impl SingleData {
    pub fn new(id: Option<MessageId>, bytes: Bytes, priority: f32) -> Self {
        Self {
            id,
            bytes,
            priority,
        }
    }

    /// Number of bytes required to serialize this message
    pub fn len(&self) -> usize {
        self.bytes.len() + self.id.map_or(1, |_| 2)
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
        let tick = reader.decode::<Option<Tick>>(Fixed)?;

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
            priority: 1.0,
            // bytes: Bytes::copy_from_slice(read_bytes),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FragmentData {
    // we always need a message_id for fragment messages, for re-assembly
    pub message_id: MessageId,
    pub fragment_id: FragmentIndex,
    pub num_fragments: FragmentIndex,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
    #[serde(skip)]
    pub priority: f32,
}

impl ToBytes for FragmentData {
    fn len(&self) -> usize {
        4 + self.bytes.len() + octets::varint_len(self.bytes.len() as u64)
    }

    fn to_bytes(&self, octets: &mut OctetsMut) -> Result<(), SerializationError> {
        octets.put_u16(self.message_id.0)?;
        octets.put_u8(self.fragment_id)?;
        octets.put_u8(self.num_fragments)?;
        octets.put_varint(self.bytes.len() as u64)?;
        octets.put_bytes(self.bytes.as_ref())?;
        Ok(())
    }

    fn from_bytes(octets: &mut octets::Octets) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let message_id = MessageId(octets.get_u16()?);
        let fragment_id = octets.get_u8()?;
        let num_fragments = octets.get_u8()?;
        let bytes = Bytes::from(octets.get_bytes(octets.cap())?);
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            bytes,
            priority: 1.0,
        })
    }
}

impl FragmentData {
    pub(crate) fn is_last_fragment(&self) -> bool {
        self.fragment_id == self.num_fragments - 1
    }
}

impl MessageContainer {
    pub(crate) fn message_id(&self) -> Option<MessageId> {
        match &self {
            MessageContainer::Single(data) => data.id,
            MessageContainer::Fragment(data) => Some(data.message_id),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::bitcode::reader::BitcodeReader;
    use crate::serialize::bitcode::writer::BitcodeWriter;

    #[test]
    fn test_serde_single_data() {
        let data = SingleData::new(Some(MessageId(1)), vec![9, 3].into(), 1.0);
        let mut writer = BitcodeWriter::with_capacity(10);
        let _a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = BitcodeReader::start_read(bytes);
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
            priority: 1.0,
        };
        let mut writer = BitcodeWriter::with_capacity(10);
        let _a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = BitcodeReader::start_read(bytes);
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
