/// Defines the [`Message`](message::Message) struct, which is a piece of serializable data
use core::fmt::Debug;


use bytes::Bytes;

use lightyear_core::tick::Tick;
use lightyear_serde::reader::{ReadVarInt, Reader};
use lightyear_serde::varint::varint_len;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_utils::wrapping_id;

// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);

/// The index of a fragment in a fragmented message.
///
/// It will be serialized as a varint, so it will take only 1 byte if there
/// are less than 64 fragments in the message.
// TODO: as an optimization, we could do a varint up to u16, so that we use 1 byte for the first 128 fragments
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FragmentIndex(pub(crate) u64);

/// Struct to keep track of which messages/slices have been received by the remote
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
#[doc(hidden)]
pub struct MessageAck {
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
#[derive(Debug, PartialEq)]
#[doc(hidden)]
pub struct SendMessage {
    pub(crate) data: MessageData,
    pub(crate) priority: f32,
}

#[derive(Debug, PartialEq)]
#[doc(hidden)]
pub struct ReceiveMessage {
    pub(crate) data: MessageData,
    // keep track on the receiver side of the sender tick when the message was actually sent
    pub(crate) remote_sent_tick: Tick,
}

#[derive(Debug, PartialEq)]
pub(crate) enum MessageData {
    Single(SingleData),
    Fragment(FragmentData),
}

#[allow(clippy::len_without_is_empty)]
impl MessageData {
    pub fn message_id(&self) -> Option<MessageId> {
        match self {
            MessageData::Single(data) => data.id,
            MessageData::Fragment(data) => Some(data.message_id),
        }
    }

    #[allow(unused)]
    pub fn set_id(&mut self, id: MessageId) {
        match self {
            MessageData::Single(data) => data.id = Some(id),
            MessageData::Fragment(data) => data.message_id = id,
        };
    }

    pub fn bytes_len(&self) -> usize {
        match self {
            MessageData::Single(data) => data.bytes_len(),
            MessageData::Fragment(data) => data.bytes_len(),
        }
    }

    #[allow(unused)]
    pub fn bytes(&self) -> Bytes {
        match self {
            MessageData::Single(data) => data.bytes.clone(),
            MessageData::Fragment(data) => data.bytes.clone(),
        }
    }
}

impl From<FragmentData> for MessageData {
    fn from(value: FragmentData) -> Self {
        Self::Fragment(value)
    }
}

impl From<SingleData> for MessageData {
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
pub(crate) struct SingleData {
    // TODO: MessageId is from 1 to 65535, so that we can use 0 to represent None? and do some bit-packing?
    pub id: Option<MessageId>,
    pub bytes: Bytes,
}

impl ToBytes for SingleData {
    // TODO: how to avoid the option taking 1 byte?
    fn bytes_len(&self) -> usize {
        self.id.bytes_len() + self.bytes.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.id.to_bytes(buffer)?;
        self.bytes.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let id = Option::<MessageId>::from_bytes(buffer)?;
        let bytes = Bytes::from_bytes(buffer)?;
        Ok(Self { id, bytes })
    }
}

impl SingleData {
    pub fn new(id: Option<MessageId>, bytes: Bytes) -> Self {
        Self { id, bytes }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FragmentData {
    // we always need a message_id for fragment messages, for re-assembly
    pub message_id: MessageId,
    pub fragment_id: FragmentIndex,
    pub num_fragments: FragmentIndex,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
}

impl ToBytes for FragmentIndex {
    fn bytes_len(&self) -> usize {
        varint_len(self.0)
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_varint(self.0)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(FragmentIndex(buffer.read_varint()?))
    }
}

impl ToBytes for FragmentData {
    fn bytes_len(&self) -> usize {
        self.message_id.bytes_len()
            + self.fragment_id.bytes_len()
            + self.num_fragments.bytes_len()
            + self.bytes.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.message_id.to_bytes(buffer)?;
        self.fragment_id.to_bytes(buffer)?;
        self.num_fragments.to_bytes(buffer)?;
        self.bytes.to_bytes(buffer)?;
        Ok(())
    }

    /// We get the FragmentData as a subslice of the original Bytes. O(1) operation.
    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let message_id = MessageId::from_bytes(buffer)?;
        let fragment_id = FragmentIndex::from_bytes(buffer)?;
        let num_fragments = FragmentIndex::from_bytes(buffer)?;
        let bytes = Bytes::from_bytes(buffer)?;
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            bytes,
        })
    }
}

impl FragmentData {
    pub(crate) fn is_last_fragment(&self) -> bool {
        self.fragment_id.0 == self.num_fragments.0 - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "std"))]
    use alloc::vec;

    #[test]
    fn test_to_bytes_single_data() {
        {
            let data = SingleData::new(None, vec![7u8; 10].into());
            let mut writer = vec![];
            data.to_bytes(&mut writer).unwrap();

            assert_eq!(writer.len(), data.bytes_len());

            let mut reader = writer.into();
            let decoded = SingleData::from_bytes(&mut reader).unwrap();
            assert_eq!(decoded, data);
        }
        {
            let data = SingleData::new(Some(MessageId(1)), vec![7u8; 10].into());
            let mut writer = vec![];
            data.to_bytes(&mut writer).unwrap();

            assert_eq!(writer.len(), data.bytes_len());

            let mut reader = writer.into();
            let decoded = SingleData::from_bytes(&mut reader).unwrap();
            assert_eq!(decoded, data);
        }
    }

    #[test]
    fn test_to_bytes_fragment_data() {
        let bytes = Bytes::from(vec![0; 10]);
        let data = FragmentData {
            message_id: MessageId(0),
            fragment_id: FragmentIndex(2),
            num_fragments: FragmentIndex(3),
            bytes: bytes.clone(),
        };
        let mut writer = vec![];
        data.to_bytes(&mut writer).unwrap();

        assert_eq!(writer.len(), data.bytes_len());

        let mut reader = writer.into();
        let decoded = FragmentData::from_bytes(&mut reader).unwrap();
        assert_eq!(decoded, data);
    }
}
