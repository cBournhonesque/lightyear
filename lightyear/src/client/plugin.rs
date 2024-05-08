//! Defines the client bevy plugin
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use crate::client::diagnostics::ClientDiagnosticsPlugin;
use crate::client::events::ClientEventsPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::networking::ClientNetworkingPlugin;
use crate::client::prediction::plugin::PredictionPlugin;
use crate::client::replication::{ClientReplicationReceivePlugin, ClientReplicationSendPlugin};
use crate::prelude::server::ServerConfig;
use crate::server::events::ServerEventsPlugin;
use crate::server::networking::ServerNetworkingPlugin;
use crate::server::replication::{ServerReplicationReceivePlugin, ServerReplicationSendPlugin};
use crate::server::visibility::immediate::VisibilityPlugin;
use crate::server::visibility::room::RoomPlugin;
use crate::shared::config::Mode;
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
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let tick_interval = self.config.shared.tick.tick_duration;
        let interpolation_config = self.config.interpolation.clone();
        builder
            .add(SetupPlugin {
                config: self.config,
            })
            .add(ClientEventsPlugin)
            .add(ClientNetworkingPlugin)
            .add(ClientDiagnosticsPlugin)
            .add(ClientReplicationReceivePlugin { tick_interval })
            .add(ClientReplicationSendPlugin { tick_interval })
            .add(PredictionPlugin)
            .add(InterpolationPlugin::new(interpolation_config))
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
            // RESOURCES //
            .insert_resource(self.config.clone());

        // TODO: how do we make sure that SharedPlugin is only added once if we want to switch between
        //  HostServer and Separate mode?
        if self.config.shared.mode == Mode::Separate {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    config: self.config.shared.clone(),
                });
        }
    }
}
