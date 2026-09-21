//! WebSocket transport wrappers for Lightyear.
//!
//! This crate adapts `aeronet_websocket` into Lightyear's transport-neutral
//! [`Link`](lightyear_link::Link) model through `lightyear_aeronet`. Client support is available
//! with the `client` feature. Accepting endpoints are available with either `p2p` or `server` on
//! non-WASM targets; `p2p` does not enable the Lightyear server role.
//!
//! Links use the default debug connection backend unless an explicit backend (such as netcode)
//! owns them or `NoDefaultConnection` disables admission. Native socket addresses supply fallback
//! routing IDs; they are not authenticated identities or an agreement between peers.
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
/// Client-side WebSocket transport integration.
pub mod client;
#[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
/// WebSocket endpoint transport integration.
pub mod endpoint;
#[cfg(all(feature = "lobby", not(target_family = "wasm")))]
/// Peer discovery over WebSocket.
pub mod lobby;

use alloc::string::String;
use bevy_ecs::prelude::Component;
use core::net::SocketAddr;

/// Observed local socket address, published after the native transport binds or connects.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebSocketLocalAddr(pub SocketAddr);

/// Observed remote socket address of a native WebSocket session.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebSocketPeerAddr(pub SocketAddr);

/// Errors produced while creating WebSocket client or server transport entities.
#[derive(thiserror::Error, Debug)]
pub enum WebSocketError {
    /// The configured certificate hash string is invalid.
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
}

/// Re-exports commonly needed by applications configuring WebSocket transport.
pub mod prelude {
    pub use crate::{WebSocketError, WebSocketLocalAddr, WebSocketPeerAddr};
    pub use aeronet_websocket::*;

    /// Client-side WebSocket prelude.
    ///
    /// Available with the `client` feature.
    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{WebSocketClientIo, WebSocketScheme};
        pub use aeronet_websocket::client::ClientConfig;
    }

    /// WebSocket endpoint prelude.
    ///
    /// Available with the `p2p` or `server` feature on non-WASM targets.
    #[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
    pub mod endpoint {
        pub use crate::endpoint::WebSocketEndpoint;
        pub use aeronet_websocket::server::ServerConfig;
    }
}
