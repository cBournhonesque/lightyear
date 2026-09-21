//! # Lightyear Steam
//!
//! This crate provides an integration layer for using Steam's networking sockets
//! (specifically `steamworks::networking_sockets`) as a transport for Lightyear.
//!
//! It handles the setup of Steam P2P connections and wraps them in a way that
//! can be used by Lightyear's `Link` component. This allows Lightyear to send
//! and receive messages over the Steam network infrastructure.
//! Native accepting endpoints are available with either `p2p` or `server`. The `p2p` feature
//! enables Steam's accepting transport without enabling the Lightyear server role.
//! [`SteamPlugin`] installs transport and connection admission together. Use
//! [`SteamTransportPlugin`] alone when another connection backend owns admission.
//!
//! Note: This crate requires the `steamworks` crate and a running Steam client.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::Res;

#[cfg(feature = "client")]
pub mod client;
#[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
pub mod endpoint;

#[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
mod connection;
#[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
pub use connection::SteamConnectionPlugin;

/// Native facts available once Steam has established a session.
///
/// `remote` is the authenticated incoming Steam identity, or the authenticated P2P dial target.
/// It is `None` for outgoing socket sessions: Aeronet currently does not expose their remote Steam
/// identity. Their native socket target remains in `SteamClientIo` and the Steam
/// connection backend uses the role-relative server routing ID, not an invented Steam identity.
#[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
#[derive(bevy_ecs::prelude::Component, Debug, Clone, Copy)]
pub struct SteamSessionFacts {
    pub local: aeronet_steam::steamworks::SteamId,
    pub remote: Option<aeronet_steam::steamworks::SteamId>,
}

/// Steam byte transport and native link lifecycle, without connection admission.
pub struct SteamTransportPlugin;

impl Plugin for SteamTransportPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
        if !app.is_plugin_added::<lightyear_aeronet::AeronetPlugin>() {
            app.add_plugins(lightyear_aeronet::AeronetPlugin);
        }
        #[cfg(feature = "client")]
        app.add_plugins(client::ClientTransportPlugin);
        #[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
        app.add_plugins(endpoint::EndpointTransportPlugin);
        let _ = app;
    }
}

/// Steam transport plus native connection admission.
pub struct SteamPlugin;

impl Plugin for SteamPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
        if !app.is_plugin_added::<SteamConnectionPlugin>() {
            app.add_plugins(SteamConnectionPlugin);
        }
        if !app.is_plugin_added::<SteamTransportPlugin>() {
            app.add_plugins(SteamTransportPlugin);
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum SteamError {}

pub mod prelude {
    pub use crate::SteamAppExt;
    pub use crate::SteamError;
    #[cfg(any(feature = "client", feature = "p2p", feature = "server"))]
    pub use crate::{SteamConnectionPlugin, SteamSessionFacts};
    pub use crate::{SteamPlugin, SteamTransportPlugin};
    pub use aeronet_steam::SessionConfig;
    pub use aeronet_steam::SteamworksClient;
    pub use aeronet_steam::steamworks;
    pub use aeronet_steam::steamworks::SteamId;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::SteamClientIo;
        pub use aeronet_steam::client::ConnectTarget;
    }

    /// Accepting endpoints, available with `p2p` or `server` on non-WASM targets.
    #[cfg(all(any(feature = "p2p", feature = "server"), not(target_family = "wasm")))]
    pub mod endpoint {
        pub use crate::endpoint::SteamEndpoint;
        pub use aeronet_steam::server::ListenTarget;
    }
}

pub trait SteamAppExt {
    /// Creates a steamworks::Client with the given app_id and adds it to the Bevy app.
    /// Then insert it as a resource as expected by `aeronet_steam`.
    ///
    /// The steam resources need to be inserted before the lightyear plugins
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
