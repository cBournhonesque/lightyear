//! Explicit server authorization, independent of transport routing.

use alloc::vec::Vec;
use core::net::SocketAddr;
use sha2::{Digest, Sha256};

/// Version of the authenticated identity-token user-data header.
pub const IDENTITY_TOKEN_VERSION: u8 = 1;
/// Bytes reserved for the version and exact server identity binding.
pub const IDENTITY_TOKEN_HEADER_BYTES: usize = 1 + 32;
/// Application data capacity of an identity token. Socket tokens retain all 256 bytes.
pub const IDENTITY_USER_DATA_BYTES: usize = crate::USER_DATA_BYTES - IDENTITY_TOKEN_HEADER_BYTES;

/// A full-width server identity binding, never a projected routing [`PeerId`](lightyear_core::id::PeerId).
///
/// The minter and server must agree on a trusted native identity. For pinned WebTransport,
/// use domain `b"webtransport-cert-sha256"` and the raw 32-byte SHA-256 certificate digest.
/// The client must independently authenticate that same certificate when dialing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityBinding([u8; 32]);

impl IdentityBinding {
    /// Derives SHA-256 over `b"lightyear-netcode-server-binding-v1\0"`, the domain's
    /// byte length as a big-endian u64, the domain bytes, and the full native identity bytes.
    /// No truncation, reserved-value remapping, or socket/hostname substitution is performed.
    pub fn from_native(domain: &[u8], identity: &[u8]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"lightyear-netcode-server-binding-v1\0");
        hash.update((domain.len() as u64).to_be_bytes());
        hash.update(domain);
        hash.update(identity);
        Self(hash.finalize().into())
    }

    /// Imports an already-derived binding, for example from an external token minter.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The only token authorization mode accepted by a server.
///
/// Socket tokens have nonempty authenticated address lists. Identity tokens have empty
/// public and private address lists and a versioned identity header in private user data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerTokenBinding {
    Socket(Vec<SocketAddr>),
    Identity(IdentityBinding),
}

impl Default for ServerTokenBinding {
    /// An unconfigured socket server fails closed; there is no unbound mode.
    fn default() -> Self {
        Self::Socket(Vec::new())
    }
}
