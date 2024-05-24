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

use crate::client::diagnostics::ClientDiagnosticsPlugin;
use crate::client::events::ClientEventsPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::networking::ClientNetworkingPlugin;
use crate::client::prediction::plugin::PredictionPlugin;
use crate::client::replication::{
    receive::ClientReplicationReceivePlugin, send::ClientReplicationSendPlugin,
};
use crate::prelude::server::{ServerConfig, ServerPlugins};
use crate::shared::config::Mode;
use crate::shared::plugin::SharedPlugin;

use super::config::{ClientConfig, ReplicationConfig};

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
    pub config: ClientConfig,
}

impl ClientPlugins {
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }
}

impl PluginGroup for ClientPlugins {
    #[allow(clippy::let_and_return)]
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let tick_interval = self.config.shared.tick.tick_duration;
        let interpolation_config = self.config.interpolation.clone();
        let builder = builder
            .add(SetupPlugin {
                config: self.config,
            })
            .add(ClientEventsPlugin)
            .add(ClientNetworkingPlugin)
            .add(ClientDiagnosticsPlugin)
            .add(ClientReplicationReceivePlugin { tick_interval })
            .add(ClientReplicationSendPlugin { tick_interval })
            .add(PredictionPlugin)
            .add(InterpolationPlugin::new(interpolation_config));

        #[cfg(target_family = "wasm")]
        let builder = builder.add(crate::client::web::WebPlugin);

        builder
    }
}

struct SetupPlugin {
    config: ClientConfig,
}

// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        app
            // REFLECTION
            .register_type::<ReplicationConfig>()
            // RESOURCES //
            .insert_resource(self.config.clone());

        // TODO: how do we make sure that SharedPlugin is only added once if we want to switch between
        //  HostServer and Separate mode?
        // if self.config.shared.mode == Mode::Separate {
        if !app.is_plugin_added::<SharedPlugin>() {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    config: self.config.shared.clone(),
                });
        }
    }
}
