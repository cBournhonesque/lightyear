//! Connection lifecycle primitives for Lightyear.
//!
//! `lightyear_connection` sits above [`lightyear_link`] and below concrete connection protocols such
//! as `lightyear_raw_connection`, `lightyear_netcode`, and `lightyear_steam`. It does not open
//! sockets or authenticate peers by itself. Instead, it provides the shared ECS vocabulary used by
//! those crates: client/server lifecycle markers, connection start/stop triggers, peer metadata, and
//! target selection helpers.
//!
//! The crate intentionally keeps long-lived connection state separate from the byte-oriented
//! [`Link`](lightyear_link::Link). A link can be established by UDP, WebSocket, WebTransport, Steam,
//! an in-process transport, or another IO backend; the connection layer records whether that link is
//! considered usable as a client, server, or server-side client connection.
//!
//! The most commonly used items are re-exported from [`prelude`].
#![no_std]

extern crate alloc;
extern crate core;

use bevy_app::{App, Plugin};
use bevy_ecs::schedule::SystemSet;

/// Client-side connection lifecycle components and observers.
pub mod client;

/// Server-side connection lifecycle components and observers.
pub mod server;

/// Direction markers for APIs that distinguish client-to-server and server-to-client traffic.
pub mod direction;
/// Peer and entity targeting helpers used by replication and message routing.
pub mod network_target;

/// Server-side client-link marker components.
pub mod client_of;
#[allow(unused)]
/// Run conditions for identifying whether the local app is acting as a client, server, or host.
pub mod identity;
/// Shared connection request and denial types.
pub mod shared;

/// Host-server marker components and observers.
pub mod host;

#[deprecated(note = "Use ConnectionSystems instead")]
/// Deprecated alias for [`ConnectionSystems`].
pub type ConnectionSet = ConnectionSystems;

/// System sets for protocol-level connection processing.
///
/// Concrete connection crates use these sets between [`LinkSystems`](lightyear_link::LinkSystems)
/// and transport systems. For example, netcode decrypts and validates bytes after link receive but
/// before transport receive, then wraps outgoing transport bytes before link send.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ConnectionSystems {
    // PreUpdate
    /// Process received link bytes before transport systems consume them.
    Receive,

    // PostUpdate
    /// Process outgoing transport bytes before link systems flush them to IO.
    Send,
}

/// Re-exports used by applications and protocol crates.
pub mod prelude {
    pub use crate::ConnectionSystems;
    pub use crate::direction::NetworkDirection;
    pub use crate::network_target::NetworkTarget;

    // we also export these types at the top level for easier access
    pub use crate::client::{
        Client, Connect, Connected, Connecting, ConnectionError, Disconnect, Disconnected,
        PeerMetadata,
    };

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{
            Client, Connect, Connected, Connecting, ConnectionError, Disconnect, Disconnected,
        };
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::client_of::ClientOf;
        pub use crate::server::{
            ConnectionError, Start, Started, Starting, Stop, Stopped, is_headless_server,
        };
    }
}

/// Root plugin for connection primitives.
///
/// The specialized client and server lifecycle observers live in
/// [`client::ConnectionPlugin`] and [`server::ConnectionPlugin`]. This root plugin currently exists
/// as a stable extension point for shared connection setup.
pub struct ConnectionPlugin;

impl Plugin for ConnectionPlugin {
    fn build(&self, _: &mut App) {}
}
