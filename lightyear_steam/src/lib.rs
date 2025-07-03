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
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

#[derive(thiserror::Error, Debug)]
pub enum SteamError {}

pub mod prelude {
    pub use crate::SteamError;
    pub use aeronet_steam::SessionConfig;
    pub use aeronet_steam::SteamworksClient;
    pub use aeronet_steam::steamworks;
    pub use aeronet_steam::steamworks::SteamId;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{SteamClientIo, SteamClientPlugin};
        pub use aeronet_steam::client::ConnectTarget;
    }

    #[cfg(all(feature = "server", not(target_family = "wasm")))]
    pub mod server {
        pub use crate::server::{SteamServerIo, SteamServerPlugin};
        pub use aeronet_steam::server::ListenTarget;
    }
}
