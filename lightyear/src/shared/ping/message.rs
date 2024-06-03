//! Defines the actual ping/pong messages
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::ping::store::PingId;
use crate::shared::time_manager::WrappedTime;
use byteorder::{ReadBytesExt, WriteBytesExt};
use std::io::Seek;

// TODO: do we need the ping ids? we could just re-use the message id ?
/// Ping message; the remote should respond immediately with a pong
#[derive(Clone, Debug, PartialEq)]
pub struct Ping {
    pub id: PingId,
}

impl ToBytes for Ping {
    fn len(&self) -> usize {
        2
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        self.id.to_bytes(buffer)
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Ping {
            id: PingId::from_bytes(buffer)?,
        })
    }
}

/// Pong message sent in response to a ping
#[derive(Clone, Debug)]
pub struct Pong {
    /// id of the ping message that triggered this pong
    pub ping_id: PingId,
    /// time when the ping was received
    pub ping_received_time: WrappedTime,
    /// time when the pong was sent
    pub pong_sent_time: WrappedTime,
}

impl ToBytes for Pong {
    fn len(&self) -> usize {
        10
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        self.ping_id.to_bytes(buffer)?;
        self.ping_received_time.to_bytes(buffer)?;
        self.pong_sent_time.to_bytes(buffer)
    }

    fn from_bytes<T: ReadBytesExt + Seek>(buffer: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Pong {
            ping_id: PingId::from_bytes(buffer)?,
            ping_received_time: WrappedTime::from_bytes(buffer)?,
            pong_sent_time: WrappedTime::from_bytes(buffer)?,
        })
    }
}
