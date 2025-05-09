use core::array::TryFromSliceError;
use no_std_io2::io as io;
use core::net::SocketAddr;

use thiserror::Error;

use crate::prelude::ClientId;

/// The result type for all the public methods that can return an error in this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// An error that can occur in the `netcode` crate.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("buffer size mismatch, expected {0} but got {1}")]
    SizeMismatch(usize, usize),
    #[error("tried to send a packet to a client {0} that doesn't exist")]
    ClientNotFound(ClientId),
    #[error("tried to send a packet to a address {0} that doesn't exist")]
    AddressNotFound(SocketAddr),
    #[error("tried to send a packet to a client that isn't connected")]
    ClientNotConnected(ClientId),
    #[error("failed to read connect token")]
    InvalidConnectToken,
    #[error("client_id {0} connect token has already been used")]
    ConnectTokenInUse(ClientId),
    #[error("client_id {0} failed to encrypt challenge token")]
    ConnectTokenEncryptionFailure(ClientId),
    #[error("failed to descrypt challenge token")]
    ConnectTokenDecryptionFailure,
    #[error(transparent)]
    UnindexableConnectToken(#[from] TryFromSliceError),
    #[error("could not parse the socket addr: {0}")]
    AddressParseError(#[from] core::net::AddrParseError),
    #[error("a client with address {0} is already connected")]
    ClientAddressInUse(SocketAddr),
    #[error("client_id {0} a client with this id is already connected")]
    ClientIdInUse(ClientId),
    #[error("client_id {0} presented challenge token for unknown id")]
    UnknownClient(ClientId),
    #[error("client_id {0} tried to connect but server is full")]
    ServerIsFull(ClientId),
    #[error("client_id {0} handle_connection_request_fn returned false")]
    Denied(ClientId),
    #[error("client_id {0} server ignored non-connection-request packet")]
    Ignored(SocketAddr),
    #[cfg(all(feature = "std", not(target_arch = "wasm32")))]
    #[error("clock went backwards (did you invent a time machine?): {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[cfg(all(feature = "std", target_arch = "wasm32"))]
    #[error("clock went backwards (did you invent a time machine?): {0}")]
    SystemTime(#[from] web_time::SystemTimeError),
    #[error("invalid connect token: {0}")]
    InvalidToken(super::token::InvalidTokenError),
    #[error(transparent)]
    Crypto(#[from] super::crypto::Error),
    #[error("invalid packet: {0}")]
    Packet(#[from] super::packet::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Transport(#[from] crate::transport::error::Error),
    #[error("client_id {0} client specific transport error {1}")]
    ClientTransport(ClientId, crate::transport::error::Error),
    #[error("address {0} address specific transport error  {1}")]
    AddressTransport(SocketAddr, crate::transport::error::Error),
}
