//! Defines the actual ping/pong messages

use crate::ping::store::PingId;
use lightyear_core::time::PositiveTickDelta;
use lightyear_serde::ToBytes;
use lightyear_serde::prelude::*;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;

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
    /// time spent between pong_send and ping_receive
    pub frame_time: PositiveTickDelta,
    // pub overstep: f32,
}

impl ToBytes for Pong {
    fn bytes_len(&self) -> usize {
        self.ping_id.bytes_len() + self.frame_time.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.ping_id.to_bytes(buffer)?;
        self.frame_time.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Pong {
            ping_id: PingId::from_bytes(buffer)?,
            frame_time: PositiveTickDelta::from_bytes(buffer)?,
        })
    }
}
