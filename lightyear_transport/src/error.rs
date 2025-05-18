use crate::channel::ChannelKind;
use crate::channel::receivers::error::ChannelReceiveError;
use crate::packet::error::PacketError;
use bytes::Bytes;
use crossbeam_channel::TrySendError;

pub type Result<T> = core::result::Result<T, TransportError>;
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error(transparent)]
    SerializationError(#[from] lightyear_serde::SerializationError),
    #[error(transparent)]
    PacketError(#[from] PacketError),
    #[error("channel {0:?} was not found")]
    ChannelNotFound(ChannelKind),
    #[error("receiver channel error: {0}")]
    ChannelReceiveError(#[from] ChannelReceiveError),
    #[error("error sending data: {0}")]
    ChannelSendError(#[from] TrySendError<(ChannelKind, Bytes, f32)>),
}
