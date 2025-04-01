//! Defines the [`ClientPlugins`] PluginGroup
//!
//! The client consists of multiple different plugins, each with their own responsibilities. These plugins
//! are grouped into the [`ClientPlugins`] plugin group, which allows you to easily configure and disable
//! any of the existing plugins.
//!
//! This means that users can simply disable existing functionality and replace it with specialized solutions,
//! while keeping the rest of the features intact.
//!
//! Most plugins are truly necessary for the server functionality to work properly, but some could be disabled.

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_shared::plugin::SharedPlugin;

/// A plugin group containing all the client plugins.
///
/// By default, the following plugins will be added:
/// - [`SetupPlugin`]: Adds the [`ClientConfig`] resource and the [`SharedPlugin`] plugin.
/// - [`ClientEventsPlugin`]: Adds the client network event
/// - [`ClientNetworkingPlugin`]: Handles the network state (connecting/disconnecting the client, sending/receiving packets)
/// - [`ClientDiagnosticsPlugin`]: Computes diagnostics about the client connection. Can be disabled if you don't need it.
/// - [`ClientReplicationReceivePlugin`]: Handles the replication of entities and resources from server to client. This can be
///   disabled if you don't need server to client replication.
/// - [`ClientReplicationSendPlugin`]: Handles the replication of entities and resources from client to server. This can be
///   disabled if you don't need client to server replication.
/// - [`PredictionPlugin`]: Handles the client-prediction systems. This can be disabled if you don't need it.
/// - [`InterpolationPlugin`]: Handles the interpolation systems. This can be disabled if you don't need it.
pub struct ClientPlugins {
    /// The tick interval for the client. This is used to determine how often the client should tick.
    /// The default value is 1/60 seconds.
    pub tick_duration: Duration,
}


impl PluginGroup for ClientPlugins {
    #[allow(clippy::let_and_return)]
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let builder = builder
            .add(SetupPlugin {
                tick_duration: self.tick_duration
            })
            .add(lightyear_sync::client::ClientPlugin);

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::client_plugin::NetcodeClientPlugin);


        #[cfg(target_family = "wasm")]
        let builder = builder.add(crate::client::web::WebPlugin);

        builder
    }
}

struct SetupPlugin {
    /// The tick interval for the client. This is used to determine how often the client should tick.
    /// The default value is 1/60 seconds.
    pub tick_duration: Duration,
}

// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<SharedPlugin>() {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    tick_duration: self.tick_duration
                });
        }
    }
}
