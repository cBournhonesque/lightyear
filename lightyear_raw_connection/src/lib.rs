//! Raw connection adapters for Lightyear links.
//!
//! Lightyear separates byte IO from long-lived connection state. [`lightyear_link`] owns the
//! byte-oriented [`Link`](lightyear_link::Link), while [`lightyear_connection`] owns client/server
//! lifecycle markers and peer identities.
//!
//! `lightyear_raw_connection` is the simplest bridge between those layers: a link becoming
//! [`Linked`](lightyear_link::Linked) is treated as the connection becoming connected or started, and
//! unlinking is treated as disconnecting or stopping. There is no handshake, authentication,
//! encryption, or protocol-level packet wrapping here.
//!
//! Use this crate when the underlying IO transport is already trusted or when tests/examples only
//! need connection lifecycle semantics. Use `lightyear_netcode` or another protocol crate when a
//! link needs authentication or session negotiation.
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "client")]
/// Client-side raw connection bridge.
pub mod client;

#[cfg(feature = "server")]
/// Server-side raw connection bridge.
pub mod server;

/// Re-exports for raw client/server connection setup.
pub mod prelude {
    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::RawClient;
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::RawServer;
    }
}
