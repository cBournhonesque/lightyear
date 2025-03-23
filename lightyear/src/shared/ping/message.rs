//! Defines the actual ping/pong messages
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::ping::store::PingId;
use crate::shared::time_manager::WrappedTime;
use crate::serialize::writer::WriteInteger;

// TODO: do we need the ping ids? we could just re-use the message id ?
/// Ping message; the remote should respond immediately with a pong
#[derive(Clone, Debug, PartialEq)]
pub struct Ping {
    pub id: PingId,
}

impl ToBytes for Ping {
    fn bytes_len(&self) -> usize {
        self.id.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.id.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
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
    fn bytes_len(&self) -> usize {
        self.ping_id.bytes_len() + self.ping_received_time.bytes_len() + self.pong_sent_time.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.ping_id.to_bytes(buffer)?;
        self.ping_received_time.to_bytes(buffer)?;
        self.pong_sent_time.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
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
