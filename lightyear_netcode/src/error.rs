use bevy::prelude::Entity;
use core::array::TryFromSliceError;
use core::net::SocketAddr;
use lightyear_core::id::PeerId;
use thiserror::Error;
use tracing::{debug, warn};

/// The result type for all the public methods that can return an error in this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// An error that can occur in the `netcode` crate.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("buffer size mismatch, expected {0} but got {1}")]
    SizeMismatch(usize, usize),
    #[error("tried to send a packet to a client {0} that doesn't exist")]
    ClientNotFound(PeerId),
    #[error("tried to send a packet to an entity {0} that doesn't exist")]
    EntityNotFound(Entity),
    #[error("tried to send a packet to a client that isn't connected")]
    ClientNotConnected(PeerId),
    #[error("failed to read connect token")]
    InvalidConnectToken,
    #[error("client_id {0} connect token has already been used")]
    ConnectTokenInUse(PeerId),
    #[error("client_id {0} failed to encrypt challenge token")]
    ConnectTokenEncryptionFailure(PeerId),
    #[error("failed to descrypt challenge token")]
    ConnectTokenDecryptionFailure,
    #[error(transparent)]
    UnindexableConnectToken(#[from] TryFromSliceError),
    #[error("could not parse the socket addr: {0}")]
    AddressParseError(#[from] core::net::AddrParseError),
    #[error("a client with entity {0} is already connected")]
    ClientEntityInUse(Entity),
    #[error("client_id {0} a client with this id is already connected")]
    ClientIdInUse(PeerId),
    #[error("client_id {0} presented challenge token for unknown id")]
    UnknownClient(PeerId),
    #[error("client_id {0} tried to connect but server is full")]
    ServerIsFull(PeerId),
    #[error("client_id {0} handle_connection_request_fn returned false")]
    Denied(PeerId),
    #[error("client_id {0} server ignored non-connection-request packet")]
    Ignored(Entity),
    #[cfg(feature = "std")]
    #[error("clock went backwards (did you invent a time machine?): {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("invalid connect token: {0}")]
    InvalidToken(super::token::InvalidTokenError),
    #[error(transparent)]
    Crypto(#[from] super::crypto::Error),
    #[error("invalid packet: {0}")]
    Packet(#[from] super::packet::Error),
    #[error(transparent)]
    Io(#[from] no_std_io2::io::Error),
    #[error(transparent)]
    Connection(#[from] lightyear_connection::client::ConnectionError),
    // #[error(transparent)]
    // Transport(#[from] crate::transport::error::Error),
    // #[error("client_id {0} client specific transport error {1}")]
    // ClientTransport(ClientId, crate::transport::error::Error),
    // #[error("address {0} address specific transport error  {1}")]
    // AddressTransport(SocketAddr, crate::transport::error::Error),
}

impl Error {
    pub(crate) fn log(self) {
        let suppress_error = match &self {
            Error::Ignored(_) => true,
            _ => false,
        };
        if suppress_error {
            debug!("Netcode error: {:?}", self);
        } else {
            warn!("Netcode error: {:?}", self);
        }
    }
}
