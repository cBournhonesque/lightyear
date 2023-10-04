use std::{error, fmt, io};

use crate::netcode::{
    client::DisconnectReason, constants::NETCODE_MAX_PAYLOAD_BYTES, token::TokenGenerationError,
};
use chacha20poly1305::aead::Error as CryptoError;

/// Errors that can happen in the netcode standard
#[derive(Debug)]
pub enum NetcodeError {
    /// No private keys was available while decrypting.
    UnavailablePrivateKey,
    /// The type of the packet is invalid.
    InvalidPacketType,
    /// The connect token has an invalid protocol id.
    InvalidProtocolID,
    /// The connect token has an invalid version.
    InvalidVersion,
    /// Packet size is too small to be a netcode packet.
    PacketTooSmall,
    /// Payload is above the maximum limit
    PayloadAboveLimit,
    /// The processed packet is duplicated
    DuplicatedSequence,
    /// No more host are available in the connect token..
    NoMoreServers,
    /// The connect token has expired.
    Expired,
    /// The client is disconnected.
    Disconnected(DisconnectReason),
    /// An error ocurred while encrypting or decrypting.
    CryptoError,
    /// The server address is not in the connect token.
    NotInHostList,
    /// Client was not found.
    ClientNotFound,
    /// Client is not connected.
    ClientNotConnected,
    /// IO error.
    IoError(io::Error),
    /// An error occured while generating the connect token.
    TokenGenerationError(TokenGenerationError),
}

impl fmt::Display for NetcodeError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use NetcodeError::*;

        match *self {
            UnavailablePrivateKey => write!(fmt, "no private key was found for this address"),
            InvalidPacketType => write!(fmt, "invalid packet type"),
            InvalidProtocolID => write!(fmt, "invalid protocol id"),
            InvalidVersion => write!(fmt, "invalid version info"),
            PacketTooSmall => write!(fmt, "packet is too small"),
            PayloadAboveLimit => write!(
                fmt,
                "payload is above the {} bytes limit",
                NETCODE_MAX_PAYLOAD_BYTES
            ),
            Expired => write!(fmt, "connection expired"),
            DuplicatedSequence => write!(fmt, "sequence already received"),
            Disconnected(reason) => write!(fmt, "disconnected: {}", reason),
            NoMoreServers => write!(fmt, "client has no more servers to connect"),
            CryptoError => write!(fmt, "error while encoding or decoding"),
            NotInHostList => write!(fmt, "token does not contain the server address"),
            ClientNotFound => write!(fmt, "client was not found"),
            ClientNotConnected => write!(fmt, "client is disconnected or connecting"),
            IoError(ref err) => write!(fmt, "{}", err),
            TokenGenerationError(ref err) => write!(fmt, "{}", err),
        }
    }
}

impl error::Error for NetcodeError {}

impl From<io::Error> for NetcodeError {
    fn from(inner: io::Error) -> Self {
        NetcodeError::IoError(inner)
    }
}

impl From<TokenGenerationError> for NetcodeError {
    fn from(inner: TokenGenerationError) -> Self {
        NetcodeError::TokenGenerationError(inner)
    }
}

impl From<CryptoError> for NetcodeError {
    fn from(_: CryptoError) -> Self {
        NetcodeError::CryptoError
    }
}
