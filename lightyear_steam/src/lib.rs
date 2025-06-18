//! # Lightyear Steam
//!
//! This crate provides an integration layer for using Steam's networking sockets
//! (specifically `steamworks::networking_sockets`) as a transport for Lightyear.
//!
//! It handles the setup of Steam P2P connections and wraps them in a way that
//! can be used by Lightyear's `Link` component. This allows Lightyear to send
//! and receive messages over the Steam network infrastructure.
//!
//! Note: This crate requires the `steamworks` crate and a running Steam client.
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

#[derive(thiserror::Error, Debug)]
pub enum SteamError {}

pub mod prelude {
    pub use crate::SteamError;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::SteamClientIo;
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::SteamServerIo;
    }
}
