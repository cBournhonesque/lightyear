use std::fmt::Debug;
use std::io::Seek;

use bevy::ecs::entity::MapEntities;
use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::{BufMut, Bytes};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use bitcode::encoding::{Fixed, Gamma};

use crate::packet::packet::FRAGMENT_SIZE;
use crate::protocol::{BitSerializable, EventContext};
use crate::serialize::reader::ReadBuffer;
use crate::serialize::varint::{varint_len, VarIntReadExt, VarIntWriteExt};
use crate::serialize::writer::WriteBuffer;
use crate::serialize::{SerializationError, ToBytes};
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

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        if let Some(id) = self.id {
            buffer.write_u8(1)?;
            buffer.write_u16(id.0)?;
        } else {
            buffer.write_u8(1)?;
        }
        buffer.write_u16(self.id.map_or(0, |id| id.0))?;
        buffer.write_varint(self.bytes.len() as u64)?;
        buffer.write(self.bytes.as_ref())?;
        Ok(())
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let id = if buffer.read_u8()? == 1 {
            Some(MessageId(buffer.read_u16()?))
        } else {
            None
        };
        let len = buffer.read_varint()? as usize;
        let mut bytes = vec![0; len];
        buffer.read_exact(&mut bytes)?;
        Ok(Self {
            id,
            bytes: Bytes::from(bytes),
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
        4 + self.bytes.len() + varint_len(self.bytes.len() as u64)
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_u16(self.message_id.0)?;
        buffer.write_u8(self.fragment_id)?;
        buffer.write_u8(self.num_fragments)?;
        buffer.write_varint(self.bytes.len() as u64)?;
        buffer.write(self.bytes.as_ref())?;
        Ok(())
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let message_id = MessageId(buffer.read_u16()?);
        let fragment_id = buffer.read_u8()?;
        let num_fragments = buffer.read_u8()?;
        let mut bytes = vec![0; buffer.read_varint()? as usize];
        buffer.read_exact(&mut bytes)?;
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            bytes: Bytes::from(bytes),
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
