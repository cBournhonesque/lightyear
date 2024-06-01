//! Errors that can happen on the client

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
    #[error(transparent)]
    Bitcode(#[from] bitcode::Error),
    #[error(transparent)]
    MessageProtocolError(#[from] crate::protocol::message::MessageError),
    #[error(transparent)]
    ComponentProtocolError(#[from] crate::protocol::component::ComponentError),
}
