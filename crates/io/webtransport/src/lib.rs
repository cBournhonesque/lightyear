//! WebTransport transport wrappers for Lightyear.
//!
//! This crate adapts `aeronet_webtransport` into Lightyear's transport-neutral
//! [`Link`](lightyear_link::Link) model through `lightyear_aeronet`. Client support is available
//! with the `client` feature. Accepting endpoints are available with either `p2p` or `server` on
//! non-WASM targets; `p2p` does not enable the Lightyear server role.
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
/// Client-side WebTransport transport integration.
pub mod client;
#[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
/// WebTransport endpoint transport integration.
pub mod endpoint;
#[cfg(all(feature = "lobby", not(target_family = "wasm")))]
/// Peer discovery over WebTransport.
pub mod lobby;

use alloc::string::String;

/// Errors produced while creating WebTransport client or server transport entities.
#[derive(thiserror::Error, Debug)]
pub enum WebTransportError {
    /// The configured certificate hash or digest string is invalid.
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
    /// A [`PeerAddr`](aeronet_io::connection::PeerAddr) component was required but missing.
    #[error("PeerAddr is required to start the WebTransportClientIo link when target is None")]
    PeerAddrMissing,
    /// A [`LocalAddr`](aeronet_io::connection::LocalAddr) component was required but missing.
    #[error("LocalAddr is required to start the WebTransportEndpoint")]
    LocalAddrMissing,
}

/// Re-exports commonly needed by applications configuring WebTransport.
pub mod prelude {
    pub use crate::WebTransportError;

    #[cfg(not(target_family = "wasm"))]
    pub use aeronet_webtransport::wtransport::Identity;

    /// Client-side WebTransport prelude.
    ///
    /// Available with the `client` feature.
    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::WebTransportClientIo;
    }

    /// WebTransport endpoint prelude.
    ///
    /// Available with the `p2p` or `server` feature on non-WASM targets.
    #[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
    pub mod endpoint {
        pub use crate::endpoint::WebTransportEndpoint;
    }
}
