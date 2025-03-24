//! Errors that can happen on the server

use crate::prelude::ClientId;

pub type Result<T> = core::result::Result<T, ServerError>;

#[derive(thiserror::Error, Debug)]
pub enum ServerError {
    #[error(transparent)]
    Networking(#[from] crate::connection::server::ConnectionError),
    #[error("could not find the server connection")]
    ServerConnectionNotFound,
    #[error("client id {0:?} was not found")]
    ClientIdNotFound(ClientId),
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
    #[error(transparent)]
    Serialization(#[from] crate::serialize::SerializationError),
    #[error(transparent)]
    MessageProtocolError(#[from] crate::protocol::message::MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] crate::protocol::component::ComponentError),
    #[error("network relevance error: {0}")]
    RelevanceError(#[from] crate::server::relevance::error::RelevanceError),
    #[error(transparent)]
    ReplicationError(#[from] crate::shared::replication::error::ReplicationError),
}
