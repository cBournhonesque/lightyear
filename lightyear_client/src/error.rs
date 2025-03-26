//! Errors that can happen on the client

use crate::serialize::SerializationError;

pub type Result<T> = core::result::Result<T, ClientError>;

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error(transparent)]
    Networking(#[from] crate::connection::client::ConnectionError),
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
    #[error(transparent)]
    Serialization(#[from] SerializationError),
    #[error(transparent)]
    MessageProtocolError(#[from] crate::protocol::message::MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] crate::protocol::component::ComponentError),
}
