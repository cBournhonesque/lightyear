//! WebSocket transport wrappers for Lightyear.
//!
//! This crate adapts `aeronet_websocket` into Lightyear's transport-neutral
//! [`Link`](lightyear_link::Link) model through `lightyear_aeronet`. Client support is available
//! with the `client` feature. Server support is available with the `server` feature on non-WASM
//! targets.
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
/// Client-side WebSocket transport integration.
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
/// Server-side WebSocket transport integration.
pub mod server;

use alloc::string::String;

/// Errors produced while creating WebSocket client or server transport entities.
#[derive(thiserror::Error, Debug)]
pub enum WebSocketError {
    /// The configured certificate hash string is invalid.
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
    /// A [`PeerAddr`](aeronet_io::connection::PeerAddr) component was required but missing.
    #[error("PeerAddr is required to start the WebSocketClientIo link")]
    PeerAddrMissing,
    /// A [`LocalAddr`](aeronet_io::connection::LocalAddr) component was required but missing.
    #[error("LocalAddr is required to start the WebSocketServerIo")]
    LocalAddrMissing,
}

/// Re-exports commonly needed by applications configuring WebSocket transport.
pub mod prelude {
    pub use crate::WebSocketError;
    pub use aeronet_websocket::*;

    /// Client-side WebSocket prelude.
    ///
    /// Available with the `client` feature.
    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{WebSocketClientIo, WebSocketScheme};
        pub use aeronet_websocket::client::ClientConfig;
    }

    /// Server-side WebSocket prelude.
    ///
    /// Available with the `server` feature on non-WASM targets.
    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::WebSocketServerIo;
        pub use aeronet_websocket::server::ServerConfig;
    }
}
