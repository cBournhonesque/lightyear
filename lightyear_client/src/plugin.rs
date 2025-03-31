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
use crate::client::message::ClientMessagePlugin;
use crate::client::networking::ClientNetworkingPlugin;
use crate::client::prediction::plugin::PredictionPlugin;
use crate::client::replication::{
    receive::ClientReplicationReceivePlugin, send::ClientReplicationSendPlugin,
};
use crate::shared::plugin::SharedPlugin;

use super::config::ClientConfig;

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
        let interpolation_config = self.config.interpolation;
        let builder = builder
            .add(SetupPlugin {
                config: self.config,
            })
            .add(lightyear_sync::client::ClientPlugin)
            .add(ClientMessagePlugin)
            .add(ClientEventsPlugin)
            .add(ClientNetworkingPlugin)
            .add(ClientDiagnosticsPlugin::default())
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
        #[cfg(feature = "metrics")]
        {
            metrics::describe_gauge!(
                "sync::prediction_time::error_ms",
                metrics::Unit::Milliseconds,
                // Ideal client time is the time that the client should be at so that the client input
                // packets arrive on time for the server to process them.
                "Difference between the actual client time and the ideal client time"
            );
            metrics::describe_counter!(
                "sync::resync_event",
                metrics::Unit::Count,
                "Resync events where the client time is resynced with the server time"
            );
            metrics::describe_gauge!(
                "inputs::input_delay_ticks",
                metrics::Unit::Count,
                "Amount of input delay applied, in ticks"
            );
        }

        app
            // RESOURCES //
            .insert_resource(self.config.clone());

        if !app.is_plugin_added::<SharedPlugin>() {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    config: self.config.shared,
                });
        }
    }
}
