//! Netcode.io connection protocol for Lightyear.
//!
//! `lightyear_netcode` implements the
//! [netcode.io protocol](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) on top
//! of an unreliable packet transport. It provides encrypted and authenticated client/server
//! sessions, then exposes accepted payload bytes back to the normal Lightyear
//! [`Link`](lightyear_link::Link) and `lightyear_transport` pipeline.
//!
//! There are two layers in this crate:
//! - [`client::Client`] and [`server::Server`] implement the protocol state machines directly.
//! - [`client_plugin::NetcodeClientPlugin`] and [`server_plugin::NetcodeServerPlugin`] adapt those
//!   state machines to Bevy entities, [`ConnectionSystems`](lightyear_connection::ConnectionSystems),
//!   and Lightyear connection markers.
//!
//! A typical production flow is:
//! 1. The client authenticates with a backend service.
//! 2. The backend creates a [`ConnectToken`] using the same `protocol_id` and private key as the
//!    game servers.
//! 3. The client constructs a [`NetcodeClient`] from [`auth::Authentication::Token`] and triggers
//!    [`Connect`](lightyear_connection::client::Connect).
//! 4. The server validates the token, creates a server-side client link, and marks it
//!    [`Connected`](lightyear_connection::client::Connected).
//! 5. Once connected, user payloads move through `lightyear_transport` while netcode wraps and
//!    unwraps the underlying link bytes.
//!
//! For local tests, [`auth::Authentication::Manual`] can build a token in-process. Production
//! clients should not have access to the private key.
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "client")]
pub use client_plugin::NetcodeClient;
pub use crypto::{Key, generate_key, try_generate_key};
pub use error::{Error, Result};
#[cfg(feature = "server")]
pub use server::{Callback, ConnectCallback, Server, ServerConfig};
#[cfg(feature = "server")]
pub use server_plugin::{NetcodeServer, TokenUserData};
pub use token::{ConnectToken, ConnectTokenBuilder, InvalidTokenError};

/// The client id from a connect token, must be unique for each client.
pub(crate) type ClientId = u64;

mod bytes;
#[cfg(feature = "client")]
/// Low-level netcode client state machine.
pub mod client;
mod crypto;
pub(crate) mod error;
mod packet;
mod replay;
#[cfg(feature = "server")]
mod server;
mod token;
mod utils;

#[cfg(feature = "client")]
/// Bevy plugin wrapper for the netcode client state machine.
pub mod client_plugin;

/// Client authentication token sources.
pub mod auth;
#[cfg(feature = "server")]
/// Bevy plugin wrapper for the netcode server state machine.
pub mod server_plugin;

/// Re-exports for Bevy application setup.
pub mod prelude {
    pub use crate::auth::Authentication;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client_plugin::{NetcodeClient, NetcodeClientPlugin, NetcodeConfig};
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server_plugin::{
            NetcodeConfig, NetcodeServer, NetcodeServerPlugin, TokenUserData,
        };
    }
}

pub(crate) const MAC_BYTES: usize = 16;
pub(crate) const MAX_PKT_BUF_SIZE: usize = 1300;
pub(crate) const CONNECTION_TIMEOUT_SEC: i32 = 15;
pub(crate) const PACKET_SEND_RATE_SEC: f64 = 1.0 / 10.0;

/// The size of a private key in bytes.
pub const PRIVATE_KEY_BYTES: usize = 32;
/// The size of the user data in a connect token in bytes.
pub const USER_DATA_BYTES: usize = 256;
/// The size of the connect token in bytes.
pub const CONNECT_TOKEN_BYTES: usize = 2048;
/// The maximum size of a packet in bytes.
pub const MAX_PACKET_SIZE: usize = 1200;
/// The version of the netcode protocol implemented by this crate.
pub const NETCODE_VERSION: &[u8; 13] = b"NETCODE 1.02\0";
