//! Replication-related errors

#[cfg(not(feature = "std"))]
use alloc::string::String;

use crate::registry::ComponentError;
use lightyear_messages::registry::MessageError;
use lightyear_serde::SerializationError;

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
}
