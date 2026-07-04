//! WebTransport transport wrappers for Lightyear.
//!
//! This crate adapts `aeronet_webtransport` into Lightyear's transport-neutral
//! [`Link`](lightyear_link::Link) model through `lightyear_aeronet`. Client support is available
//! with the `client` feature. Server support is available with the `server` feature on non-WASM
//! targets.
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
/// Client-side WebTransport transport integration.
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
/// Server-side WebTransport transport integration.
pub mod server;

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
    #[error("LocalAddr is required to start the WebTransportServerIo")]
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

    /// Server-side WebTransport prelude.
    ///
    /// Available with the `server` feature on non-WASM targets.
    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::WebTransportServerIo;
    }
}
