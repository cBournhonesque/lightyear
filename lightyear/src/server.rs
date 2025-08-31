//! Defines the Server PluginGroup
//!
//! The Server consists of multiple different plugins, each with their own responsibilities. These plugins
//! are grouped into the [`ServerPlugins`] plugin group, which allows you to easily configure and disable
//! any of the existing plugins.
//!
//! This means that users can simply disable existing functionality and replace it with specialized solutions,
//! while keeping the rest of the features intact.
//!
//! Most plugins are truly necessary for the server functionality to work properly, but some could be disabled.
use bevy_app::{PluginGroup, PluginGroupBuilder};

use crate::shared::SharedPlugins;
use core::time::Duration;

/// A plugin group containing all the server plugins.
///
/// The order in which the plugins are added matters!
/// You need to add:
/// - first add the `ServerPlugins`
/// - then build your protocol (usually in a `ProtocolPlugin`)
/// - then spawn your `Server` entity
///
/// By default, the following plugins will be added:
///   IO
/// - [`ServerLinkPlugin`](lightyear_link::server::ServerLinkPlugin): Handles how the server reacts to links getting established/disconnected
///   CONNECTION
/// -
///   MESSAGE
/// - [`MessagePlugin`](lightyear_messages::plugin::MessagePlugin): Handles the messaging system.
/// - [`ConnectionPlugin`](lightyear_connection::ConnectionPlugin): Handles connections, which are long-term connections with a persistent id on top of a link
///   REPLICATION
pub struct ServerPlugins {
    /// The tick interval for the server. This is used to determine how often the server should tick.
    /// The default value is 1/60 seconds.
    pub tick_duration: Duration,
}

impl Default for ServerPlugins {
    fn default() -> Self {
        Self {
            tick_duration: Duration::from_secs_f32(1.0 / 60.0),
        }
    }
}

impl PluginGroup for ServerPlugins {
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let builder = builder
            .add(lightyear_sync::server::ServerPlugin)
            .add(lightyear_link::server::ServerLinkPlugin);

        let builder = builder.add(SharedPlugins {
            tick_duration: self.tick_duration,
        });

        #[cfg(feature = "deterministic")]
        let builder =
            builder.add(lightyear_deterministic_replication::prelude::ChecksumReceivePlugin);

        let builder = builder.add(lightyear_connection::host::HostPlugin);

        #[cfg(feature = "replication")]
        let builder = builder.add(lightyear_replication::host::HostServerPlugin);

        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::server::ServerPlugin);

        // IO
        #[cfg(all(feature = "udp", not(target_family = "wasm")))]
        let builder = builder.add(lightyear_udp::server::ServerUdpPlugin);
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        let builder = builder.add(lightyear_webtransport::server::WebTransportServerPlugin);
        #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
        let builder = builder.add(lightyear_websocket::server::WebSocketServerPlugin);
        #[cfg(all(feature = "steam", not(target_family = "wasm")))]
        let builder = builder.add(lightyear_steam::server::SteamServerPlugin);

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::server_plugin::NetcodeServerPlugin);
        builder
    }
}
