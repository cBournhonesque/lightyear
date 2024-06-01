//! Errors for building packets

use crate::channel::receivers::error::ChannelReceiveError;
use crate::serialize::SerializationError;

pub type Result<T> = core::result::Result<T, PacketError>;
#[derive(thiserror::Error, Debug)]
pub enum PacketError {
    #[error("serialization error: {0}")]
    Serialization(#[from] SerializationError),
    #[error("channel was not found")]
    ChannelNotFound,
    #[error("receiver channel error: {0}")]
    ChannelReceiveError(#[from] ChannelReceiveError),
}
