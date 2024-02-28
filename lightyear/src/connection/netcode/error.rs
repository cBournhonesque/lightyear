use std::net::SocketAddr;
use std::fmt;
use thiserror::Error;

/// The result type for all the public methods that can return an error in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An error that can occur in the `netcode` crate.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("buffer size mismatch, expected {0} but got {1}")]
    SizeMismatch(usize, usize),
    #[error("tried to send a packet to a client that doesn't exist")]
    ClientNotFound,
    #[error("tried to send a packet to a client that isn't connected")]
    ClientNotConnected,
    #[error("clock went backwards (did you invent a time machine?): {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("invalid connect token: {0}")]
    InvalidToken(super::token::InvalidTokenError),
    #[error(transparent)]
    Crypto(#[from] super::crypto::Error),
    #[error("invalid packet: {0}")]
    Packet(#[from] super::packet::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct DisConnectionError {
    pub addr: SocketAddr,
}

impl fmt::Display for DisConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error with socket address: {:?}", self.addr)
    }
}

impl std::error::Error for DisConnectionError {}
