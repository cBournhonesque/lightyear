use crate::shared::SharedPlugins;
use bevy::app::{PluginGroup, PluginGroupBuilder};
use core::time::Duration;

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
        let builder = builder.add(lightyear_sync::client::ClientPlugin);
        let builder = builder.add_group(SharedPlugins {
            tick_duration: self.tick_duration
        });

        // CONNECTION
        #[cfg(feature = "netcode")]
        let builder = builder.add(lightyear_netcode::client_plugin::NetcodeClientPlugin);

        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::plugin::PredictionPlugin);

        #[cfg(target_family = "wasm")]
        let builder = builder.add(crate::client::web::WebPlugin);

        builder
    }
}