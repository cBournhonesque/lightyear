//! Replication-related errors

use crate::serialize::SerializationError;

pub type Result<T> = std::result::Result<T, ReplicationError>;

#[derive(thiserror::Error, Debug)]
pub enum ReplicationError {
    #[error("DeltaCompressionError: {0}")]
    DeltaCompressionError(String),
    #[error(transparent)]
    Serialization(#[from] SerializationError),
    #[error(transparent)]
    MessageProtocolError(#[from] crate::protocol::message::MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] crate::protocol::component::ComponentError),
}
