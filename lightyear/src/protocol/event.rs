use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use byteorder::{ReadBytesExt, WriteBytesExt};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EventReplicationMode {
    /// TODO: Maybe also allow events to be replicated as normal messages? we would need to:
    ///  - instead of 'register_event', just add `is_event` to MessageRegistration
    ///  - in the serialize_function, check if the message type is MessageType::Event, in which case we would
    ///    use an EventReplicationMode::None
    // /// Simply replicate the event as a normal message
    // None,
    /// Replicate the event and buffer it via an EventWriter
    Buffer,
    /// Replicate the event and trigger it
    Trigger,
}

impl ToBytes for EventReplicationMode {
    fn len(&self) -> usize {
        1
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        match self {
            // EventReplicationMode::None => buffer.write_u8(0)?,
            EventReplicationMode::Buffer => buffer.write_u8(1)?,
            EventReplicationMode::Trigger => buffer.write_u8(2)?,
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mode = buffer.read_u8()?;
        match mode {
            // 0 => Ok(EventReplicationMode::None),
            1 => Ok(EventReplicationMode::Buffer),
            2 => Ok(EventReplicationMode::Trigger),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}
