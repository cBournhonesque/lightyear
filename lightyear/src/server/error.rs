//! Errors that can happen on the server

use crate::prelude::ClientId;

pub type Result<T> = std::result::Result<T, ServerError>;

#[derive(thiserror::Error, Debug)]
pub enum ServerError {
    // TODO: add a thiserror for network connections
    #[error("network connection error")]
    NetworkConnectionError,
    #[error("could not find the server connection")]
    ServerConnectionNotFound,
    #[error("client id {0:?} was not found")]
    ClientIdNotFound(ClientId),
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
    #[error(transparent)]
    Bitcode(#[from] bitcode::Error),
    #[error(transparent)]
    MessageProtocolError(#[from] crate::protocol::message::MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] crate::protocol::component::ComponentError),
    #[error("visibility error: {0}")]
    VisibilityError(#[from] crate::server::visibility::error::VisibilityError),
}
