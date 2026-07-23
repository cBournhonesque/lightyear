/// Defines the messages that a packet will be split into
use core::fmt::Debug;

use bytes::Bytes;

use lightyear_core::tick::Tick;
use lightyear_serde::reader::{ReadVarInt, Reader};
use lightyear_serde::varint::varint_len;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_utils::wrapping_id;

use crate::channel::registry::{ChannelId, ChannelKind};
use crate::packet::compression::CompressionConfig;

// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);

/// The index of a fragment in a fragmented message.
///
/// It will be serialized as a varint, so it will take only 1 byte if there
/// are less than 64 fragments in the message.
// TODO: as an optimization, we could do a varint up to u16, so that we use 1 byte for the first 128 fragments
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FragmentIndex(pub(crate) u64);

/// Identifies a pending message inside its channel for the duration of one send flush.
///
/// Queue indices remain valid because unreliable channels only compact their queues after packet
/// staging and commit have completed. Reliable messages already have a stable [`MessageId`].
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendMessageKey {
    UnreliableSingle(usize),
    UnreliableFragment(usize),
    ReliableSingle(MessageId),
    ReliableFragment(MessageId, FragmentIndex),
}

impl SendMessageKey {
    fn unreliable_send_order(self) -> Option<u64> {
        match self {
            Self::UnreliableSingle(index) | Self::UnreliableFragment(index) => Some(index as u64),
            Self::ReliableSingle(_) | Self::ReliableFragment(_, _) => None,
        }
    }

    pub(crate) fn packing_tiebreaker(self) -> (u8, u64) {
        match self {
            Self::UnreliableSingle(_) => (0, 0),
            Self::UnreliableFragment(_) => (1, 0),
            Self::ReliableSingle(_) => (2, 0),
            Self::ReliableFragment(_, fragment_id) => (3, fragment_id.0),
        }
    }
}

/// A cheap snapshot of a channel-owned message which is eligible for packet staging.
///
/// Creating a candidate clones only its [`Bytes`] handle. The underlying message allocation
/// remains owned by the channel until
/// [`ChannelSend::commit_send`](crate::channel::send::ChannelSend::commit_send) is called after
/// the final packet enters `Link.send`.
#[derive(Debug)]
pub(crate) struct SendCandidate {
    pub(crate) channel_kind: ChannelKind,
    pub(crate) channel_id: ChannelId,
    pub(crate) key: SendMessageKey,
    /// Position in the channel's current pending order, independent of wrapping IDs.
    pub(crate) send_order: u64,
    pub(crate) message: SendMessage,
    pub(crate) effective_priority: f32,
}

impl SendCandidate {
    pub(crate) fn new(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        key: SendMessageKey,
        message: SendMessage,
    ) -> Self {
        let send_order = key
            .unreliable_send_order()
            .expect("reliable candidates require an explicit non-wrapping send order");
        Self::new_with_send_order(channel_kind, channel_id, key, send_order, message)
    }

    pub(crate) fn new_reliable(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        key: SendMessageKey,
        send_order: u64,
        message: SendMessage,
    ) -> Self {
        debug_assert!(matches!(
            key,
            SendMessageKey::ReliableSingle(_) | SendMessageKey::ReliableFragment(_, _)
        ));
        Self::new_with_send_order(channel_kind, channel_id, key, send_order, message)
    }

    fn new_with_send_order(
        channel_kind: ChannelKind,
        channel_id: ChannelId,
        key: SendMessageKey,
        send_order: u64,
        message: SendMessage,
    ) -> Self {
        Self {
            channel_kind,
            channel_id,
            key,
            send_order,
            effective_priority: message.priority,
            message,
        }
    }
}

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
#[derive(Clone, Debug, PartialEq)]
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
    pub(crate) compression: CompressionConfig,
}

#[derive(Clone, Debug, PartialEq)]
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

    pub fn bytes_len(&self) -> usize {
        match self {
            MessageData::Single(data) => data.bytes_len(),
            MessageData::Fragment(data) => data.bytes_len(),
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
    /// Compression mode for the reassembled message. Serialized only on fragment 0.
    pub compression: Option<FragmentCompression>,
    /// Bytes data associated with the message that is too big
    pub bytes: Bytes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FragmentCompression {
    #[default]
    None,
    Lz4,
}

impl FragmentCompression {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
        }
    }
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
            + if self.is_initial_fragment() {
                FragmentCompression::None.bytes_len()
            } else {
                0
            }
            + self.bytes.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.message_id.to_bytes(buffer)?;
        self.fragment_id.to_bytes(buffer)?;
        self.num_fragments.to_bytes(buffer)?;
        if self.is_initial_fragment() {
            self.compression.unwrap_or_default().to_bytes(buffer)?;
        }
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
        let compression = if fragment_id.0 == 0 {
            Some(FragmentCompression::from_bytes(buffer)?)
        } else {
            None
        };
        let bytes = Bytes::from_bytes(buffer)?;
        Ok(Self {
            message_id,
            fragment_id,
            num_fragments,
            compression,
            bytes,
        })
    }
}

impl ToBytes for FragmentCompression {
    fn bytes_len(&self) -> usize {
        1
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match self {
            Self::None => 0u8.to_bytes(buffer)?,
            Self::Lz4 => 1u8.to_bytes(buffer)?,
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        match u8::from_bytes(buffer)? {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}

impl FragmentData {
    pub(crate) fn is_initial_fragment(&self) -> bool {
        self.fragment_id.0 == 0
    }

    pub(crate) fn is_last_fragment(&self) -> bool {
        self.fragment_id.0 == self.num_fragments.0 - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn message_id_arithmetic_and_order_wrap_at_u32_max() {
        let last = MessageId(u32::MAX);
        let first = MessageId(0);
        let mut next = last;

        next += 1;

        assert_eq!(next, first);
        assert_eq!(last + MessageId(1), first);
        assert_eq!(first - 1, last);
        assert_eq!(first - last, 1);
        assert!(last < first);
        assert!(first < MessageId(1));
    }

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
            compression: None,
            bytes: bytes.clone(),
        };
        let mut writer = vec![];
        data.to_bytes(&mut writer).unwrap();

        assert_eq!(writer.len(), data.bytes_len());

        let mut reader = writer.into();
        let decoded = FragmentData::from_bytes(&mut reader).unwrap();
        assert_eq!(decoded, data);

        let initial_data = FragmentData {
            message_id: MessageId(0),
            fragment_id: FragmentIndex(0),
            num_fragments: FragmentIndex(3),
            compression: Some(FragmentCompression::Lz4),
            bytes,
        };
        let mut writer = vec![];
        initial_data.to_bytes(&mut writer).unwrap();

        assert_eq!(writer.len(), initial_data.bytes_len());
        assert_eq!(initial_data.bytes_len(), data.bytes_len() + 1);

        let mut reader = writer.into();
        let decoded = FragmentData::from_bytes(&mut reader).unwrap();
        assert_eq!(decoded, initial_data);
    }
}
