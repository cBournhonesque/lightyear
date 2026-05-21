//! Errors for building packets

use crate::channel::receivers::error::ChannelReceiveError;
use lightyear_serde::SerializationError;

pub type Result<T> = core::result::Result<T, PacketError>;

/// Errors produced while parsing or building transport packets.
#[derive(thiserror::Error, Debug)]
pub enum PacketError {
    /// Packet serialization/deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] SerializationError),
    /// A packet referenced an unknown channel.
    #[error("channel was not found")]
    ChannelNotFound,
    /// A channel receiver rejected a decoded message.
    #[error("receiver channel error: {0}")]
    ChannelReceiveError(#[from] ChannelReceiveError),
}
