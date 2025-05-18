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
use crate::shared::SharedPlugins;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;

/// A plugin group containing all the server plugins.
///
/// By default, the following plugins will be added:
/// - [`SetupPlugin`]: Adds the [`ServerConfig`] resource and the [`SharedPlugin`] plugin.
/// - [`ServerEventsPlugin`]: Adds the server network event
/// - [`ServerNetworkingPlugin`]: Handles the network state (starting/stopping the server, sending/receiving packets)
/// - [`NetworkRelevancePlugin`]: Handles the network relevance systems. This can be disabled if you don't need fine-grained interest management.
/// - [`RoomPlugin`]: Handles the room system, which is an addition to the visibility system. This can be disabled if you don't need rooms.
/// - [`ServerReplicationReceivePlugin`]: Handles the replication of entities and resources from clients to the server. This can be
///   disabled if you don't need client to server replication.
/// - [`ServerReplicationSendPlugin`]: Handles the replication of entities and resources from the server to the client. This can be
///   disabled if you don't need server to client replication.
#[derive(Default)]
pub struct ServerPlugins {
    /// The tick interval for the server. This is used to determine how often the server should tick.
    /// The default value is 1/60 seconds.
    pub tick_duration: Duration,
}

impl PluginGroup for ServerPlugins {
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let builder = builder
            .add(lightyear_sync::server::ServerPlugin)
            .add(lightyear_link::server::ServerLinkPlugin);

        let builder = builder.add_group(SharedPlugins {
            tick_duration: self.tick_duration,
        });

        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::server::ServerPlugin);

        // IO
        #[cfg(feature = "udp")]
        let builder = builder.add(lightyear_udp::server::ServerUdpPlugin);
        #[cfg(feature = "webtransport")]
        let builder = builder.add(lightyear_webtransport::server::WebTransportServerPlugin);

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::server_plugin::NetcodeServerPlugin);
        builder
    }
}
