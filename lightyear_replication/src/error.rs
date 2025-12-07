//! Replication-related errors

use alloc::string::String;

use crate::registry::ComponentError;
use lightyear_messages::registry::MessageError;
use lightyear_serde::SerializationError;
use lightyear_transport::error::TransportError;

pub type Result<T> = core::result::Result<T, ReplicationError>;

#[derive(thiserror::Error, Debug)]
pub enum ReplicationError {
    #[error("DeltaCompressionError: {0}")]
    DeltaCompressionError(String),
    #[error(transparent)]
    Serialization(#[from] SerializationError),
    #[error(transparent)]
    MessageProtocolError(#[from] MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] ComponentError),
    #[error(transparent)]
    TransportError(#[from] TransportError),
    #[cfg(feature = "std")]
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}
