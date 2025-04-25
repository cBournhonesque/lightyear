use crate::prelude::client::RemoteTimeline;
use crate::prelude::InputTimeline;
use crate::shared::SharedPlugin;
use bevy::app::{App, Plugin, PluginGroup, PluginGroupBuilder};
use bevy::prelude::Component;
use core::time::Duration;
use lightyear_connection::client::Client;
use lightyear_messages::MessageManager;

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

        // PREDICTION
        #[cfg(feature = "prediction")]
        let builder = builder.add(lightyear_prediction::plugin::PredictionPlugin);


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

        app.register_required_components::<Client, RemoteTimeline>();
        app.register_required_components::<Client, InputTimeline>();
        app.register_required_components::<Client, MessageManager>();
        #[cfg(feature = "interpolation")]
        app.register_required_components::<Client, lightyear_sync::prelude::client::InterpolationTimeline>();

        if !app.is_plugin_added::<SharedPlugin>() {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    tick_duration: self.tick_duration
                });
        }
    }
}