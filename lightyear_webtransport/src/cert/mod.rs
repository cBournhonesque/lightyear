//! Utilities for working with X.509 certificates.

#[cfg(not(target_family = "wasm"))]
mod native;

#[cfg(not(target_family = "wasm"))]
pub use native::*;
use {
    base64::{Engine, engine::general_purpose::STANDARD as BASE64},
    derive_more::{Display, Error},
};

/// Bytes representing the SHA-256 digest of the DER encoding of a certificate.
pub type CertificateHash = [u8; 32];

/// Failed to decode a [`CertificateHash`] from a base 64 string.
#[derive(Debug, Clone, Display, Error)]
pub enum DecodeHashError {
    /// Failed to decode the string from base 64.
    #[display("failed to decode into base 64")]
    Base64(base64::DecodeError),
    /// Decoded base 64 bytes were not of the same length as [`CertificateHash`]
    /// requires.
    #[display("wrong number of bytes")]
    InvalidLength,
}

/// Decodes a base 64 string produced by `hash_to_b64` into a
/// [`CertificateHash`].
///
/// This can be used as the value of a server certificate hash when configuring
/// a WebTransport endpoint on WASM.
///
/// # Errors
///
/// Errors if the input does not represent a valid certificate hash.
pub fn hash_from_b64(input: &str) -> Result<CertificateHash, DecodeHashError> {
    let hash = BASE64.decode(input).map_err(DecodeHashError::Base64)?;
    let hash = CertificateHash::try_from(hash).map_err(|_| DecodeHashError::InvalidLength)?;
    Ok(hash)
}
