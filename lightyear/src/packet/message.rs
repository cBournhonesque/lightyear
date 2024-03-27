use std::fmt::Debug;

use bytes::Bytes;

use bitcode::encoding::{Fixed, Gamma};

use crate::packet::packet::FRAGMENT_SIZE;
use crate::prelude::LightyearMapEntities;
use crate::protocol::EventContext;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::tick_manager::Tick;
use crate::utils::named::Named;
use crate::utils::wrapping_id::wrapping_id;
use bevy::ecs::entity::MapEntities;

// strategies to avoid copying:
// - have a net_id for each message or component
//   be able to take a reference to a M and serialize it into bytes (so we can cheaply clone)
//   in the serialized message, include the net_id (for decoding)
//   no bit padding

// Internal id that we assign to each message sent over the network
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

#[derive(Clone, Debug, PartialEq)]
/// This structure contains the bytes for a single 'logical' message
///
/// We store the bytes instead of the message directly.
/// This lets us serialize the message very early and then pass it around with cheap clones
/// The message/component does not need to implement Clone anymore!
/// Also we know the size of the message early, which is useful for fragmentation.
pub struct SingleData {
    pub id: Option<MessageId>,
    // TODO: for now, we include the tick into every single data because it's easier
    //  for optimizing size later, we want the tick channels to use a different SingleData type that contains tick
    //  basically each channel should have either SingleData or TickData as associated type ?
    // NOTE: This is only used for tick buffered receiver, so that the message is read at the same exact tick it was sent
    /// This tick is used to track the tick of the sender when they intended to send the message.
    /// (before priority, it was guaranteed that this tick would be same as the packet send tick, but now it's not,
    /// because you could intend to send a message on tick 7 (i.e. containing your local world update at tick 7),
    /// but the message only gets sent on tick 10 because of priority buffering).
    pub tick: Option<Tick>,
    pub bytes: Bytes,
    // we do not encode the priority in the packet
    pub priority: f32,
}

impl SingleData {
    pub fn new(id: Option<MessageId>, bytes: Bytes, priority: f32) -> Self {
        Self {
            id,
            tick: None,
            bytes,
            priority,
        }
    }

    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.encode(&self.id, Fixed)?;
        writer.encode(&self.tick, Fixed)?;
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
            tick,
            bytes: Bytes::from(read_bytes),
            priority: 1.0,
            // bytes: Bytes::copy_from_slice(read_bytes),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentData {
    // we always need a message_id for fragment messages, for re-assembly
    pub message_id: MessageId,
    /// This tick is used to track the tick of the sender when they intended to send the message.
    /// (before priority, it was guaranteed that this tick would be same as the packet send tick, but now it's not,
    /// because you could intend to send a message on tick 7 (i.e. containing your local world update at tick 7),
    /// but the message only gets sent on tick 10 because of priority buffering).
    pub tick: Option<Tick>,
    pub fragment_id: FragmentIndex,
    pub num_fragments: FragmentIndex,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
    pub priority: f32,
}

impl FragmentData {
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        let num_bits_before = writer.num_bits_written();
        writer.encode(&self.message_id, Fixed)?;
        writer.encode(&self.tick, Fixed)?;
        writer.encode(&self.fragment_id, Gamma)?;
        writer.encode(&self.num_fragments, Gamma)?;
        // TODO: be able to just concat the bytes to the buffer?
        if self.is_last_fragment() {
            // writing the slice includes writing the length of the slice
            writer.encode(self.bytes.as_ref(), Fixed)?;
            // writer.serialize(&self.bytes.to_vec());
            // writer.serialize(&self.fragment_message_bytes.as_ref());
        } else {
            let bytes_array: [u8; FRAGMENT_SIZE] = self.bytes.as_ref().try_into().unwrap();
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            writer.encode(&bytes_array, Fixed)?;
        }
        let num_bits_written = writer.num_bits_written() - num_bits_before;
        Ok(num_bits_written)
    }

    pub(crate) fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let message_id = reader.decode::<MessageId>(Fixed)?;
        let tick = reader.decode::<Option<Tick>>(Fixed)?;
        let fragment_id = reader.decode::<FragmentIndex>(Gamma)?;
        let num_fragments = reader.decode::<FragmentIndex>(Gamma)?;
        let bytes = if fragment_id == num_fragments - 1 {
            // let num_bytes = reader.decode::<usize>(Gamma)?;
            // let num_bytes_non_zero = std::num::NonZeroUsize::new(num_bytes)
            //     .ok_or_else(|| anyhow::anyhow!("num_bytes is 0"))?;
            // let read_bytes = reader.read_bytes(num_bytes_non_zero)?;
            // reader.
            // let read_bytes = reader.decode::<&[u8]>()?;
            // TODO: avoid the extra copy
            //  - maybe have the encoding of bytes be
            let read_bytes = reader.decode::<Vec<u8>>(Fixed)?;
            Bytes::from(read_bytes)
        } else {
            // Serde does not handle arrays well (https://github.com/serde-rs/serde/issues/573)
            let read_bytes = reader.decode::<[u8; FRAGMENT_SIZE]>(Fixed)?;
            // TODO: avoid the extra copy
            let bytes_vec: Vec<u8> = read_bytes.to_vec();
            Bytes::from(bytes_vec)
        };
        Ok(Self {
            message_id,
            tick,
            fragment_id,
            num_fragments,
            bytes,
            // we can assign a random priority on the reader side
            priority: 1.0,
        })
    }

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

    /// Serialize the message into a bytes buffer
    /// Returns the number of bits written
    pub(crate) fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<usize> {
        match &self {
            MessageContainer::Single(data) => data.encode(writer),
            MessageContainer::Fragment(data) => data.encode(writer),
        }
    }

    pub fn set_id(&mut self, id: MessageId) {
        match self {
            MessageContainer::Single(data) => data.id = Some(id),
            MessageContainer::Fragment(data) => data.message_id = id,
        };
    }

    /// Set the tick of the remote when this message was sent
    ///
    /// Note that in some cases the tick can be already set (for example for tick-buffered channel,
    /// for reliable channels, or for priority-related buffering, since the tick set in the packet header might not
    /// correspond to the tick when the message was initially set)
    /// In those cases we don't override the tick
    pub fn set_tick(&mut self, tick: Tick) {
        match self {
            MessageContainer::Single(data) => {
                if data.tick.is_none() {
                    data.tick = Some(tick);
                }
            }
            MessageContainer::Fragment(data) => {
                if data.tick.is_none() {
                    data.tick = Some(tick);
                }
            }
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

// TODO: for now messages must be able to be used as events, since we output them in our message events
/// A [`Message`] is basically any type that can be (de)serialized over the network.
///
/// Every type that can be sent over the network must implement this trait.
///
pub trait Message: EventContext + Named + LightyearMapEntities {}
impl<T: EventContext + Named + LightyearMapEntities> Message for T {}

#[cfg(test)]
mod tests {
    use crate::_reexport::{ReadWordBuffer, WriteWordBuffer};

    use super::*;

    #[test]
    fn test_serde_single_data() {
        let data = SingleData::new(Some(MessageId(1)), vec![9, 3].into(), 1.0);
        let mut writer = WriteWordBuffer::with_capacity(10);
        let _a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes);
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
            tick: None,
            fragment_id: 2,
            num_fragments: 3,
            bytes: bytes.clone(),
            priority: 1.0,
        };
        let mut writer = WriteWordBuffer::with_capacity(10);
        let _a = data.encode(&mut writer).unwrap();
        // dbg!(a);
        let bytes = writer.finish_write();

        let mut reader = ReadWordBuffer::start_read(bytes);
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
