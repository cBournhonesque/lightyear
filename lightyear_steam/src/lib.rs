//! Steam networking integration for Lightyear.
//!
//! This crate connects `aeronet_steam` sessions to Lightyear's link and connection layers. Steam
//! networking provides the underlying peer/session transport; Lightyear wraps those sessions in
//! [`Link`](lightyear_link::Link) entities and maps Steam IDs into
//! [`PeerId::Steam`](lightyear_core::id::PeerId::Steam).
//!
//! Client and server support are feature-gated:
//! - `client` exposes [`client::SteamClientPlugin`] and [`client::SteamClientIo`].
//! - `server` exposes [`server::SteamServerPlugin`] and [`server::SteamServerIo`] on non-Wasm
//!   targets.
//!
//! Applications must initialize Steam resources with [`SteamAppExt::add_steam_resources`] before
//! adding the Lightyear Steam plugins. A running Steam client is required by the underlying
//! `steamworks` integration.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_app::PreUpdate;
use bevy_ecs::prelude::Res;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(feature = "server", not(target_family = "wasm")))]
pub mod server;

/// Errors produced by the Steam integration layer.
///
/// The enum is currently empty because setup and session errors are surfaced by `aeronet_steam` and
/// Bevy observers.
#[derive(thiserror::Error, Debug)]
pub enum SteamError {}

/// Re-exports for Steam client/server setup.
pub mod prelude {
    pub use crate::SteamAppExt;
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

pub trait SteamAppExt {
    /// Creates a `steamworks::Client` for `app_id` and inserts the resource expected by
    /// `aeronet_steam`.
    ///
    /// The Steam resources must be inserted before adding Lightyear Steam client/server plugins.
    /// This also registers a `PreUpdate` system that runs Steam callbacks.
    fn add_steam_resources(&mut self, app_id: u32) -> &mut Self;
}

impl SteamAppExt for bevy_app::App {
    fn add_steam_resources(&mut self, app_id: u32) -> &mut Self {
        let steam =
            prelude::steamworks::Client::init_app(app_id).expect("failed to initialize steam");
        steam.networking_utils().init_relay_network_access();

        self.insert_resource(prelude::SteamworksClient(steam))
            .add_systems(PreUpdate, |steam: Res<prelude::SteamworksClient>| {
                steam.run_callbacks();
            });
        self
    }
}
