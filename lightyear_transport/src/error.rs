use crate::channel::ChannelKind;
use crate::channel::receivers::error::ChannelReceiveError;
use crate::channel::registry::ChannelId;
use crate::packet::error::PacketError;
use bytes::Bytes;
use crossbeam_channel::TrySendError;

pub type Result<T> = core::result::Result<T, TransportError>;

/// Errors produced while packetizing, sending, or receiving transport messages.
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    /// Error while serializing or deserializing packet/message bytes.
    #[error(transparent)]
    SerializationError(#[from] lightyear_serde::SerializationError),
    /// Packet builder/parser error.
    #[error(transparent)]
    PacketError(#[from] PacketError),
    /// A type-based channel was required but this transport did not have it.
    #[error("channel {0:?} was not found")]
    ChannelNotFound(ChannelKind),
    /// A network channel ID was required but was not registered.
    #[error("channel {0:?} was not found")]
    ChannelIdNotFound(ChannelId),
    /// Channel receiver rejected the received message.
    #[error("receiver channel error: {0}")]
    ChannelReceiveError(#[from] ChannelReceiveError),
    /// Internal enqueue channel rejected the message.
    #[error("error sending data: {0}")]
    ChannelSendError(#[from] TrySendError<(ChannelKind, Bytes, f32)>),
}
