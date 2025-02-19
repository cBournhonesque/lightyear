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
use crate::server::clients::ClientsMetadataPlugin;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use crate::server::events::ServerEventsPlugin;
use crate::server::message::ServerMessagePlugin;
use crate::server::networking::ServerNetworkingPlugin;
use crate::server::relevance::immediate::NetworkRelevancePlugin;
use crate::server::relevance::room::RoomPlugin;
use crate::server::replication::{
    receive::ServerReplicationReceivePlugin, send::ServerReplicationSendPlugin,
};
use crate::shared::plugin::SharedPlugin;

use super::config::ServerConfig;

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
    pub config: ServerConfig,
}

impl ServerPlugins {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }
}

impl PluginGroup for ServerPlugins {
    fn build(self) -> PluginGroupBuilder {
        let builder = PluginGroupBuilder::start::<Self>();
        let tick_interval = self.config.shared.tick.tick_duration;
        builder
            .add(SetupPlugin {
                config: self.config,
            })
            .add(ServerMessagePlugin)
            .add(ServerEventsPlugin)
            .add(ServerNetworkingPlugin)
            .add(NetworkRelevancePlugin)
            .add(RoomPlugin)
            .add(ClientsMetadataPlugin)
            .add(ServerReplicationReceivePlugin { tick_interval })
            .add(ServerReplicationSendPlugin { tick_interval })
    }
}

/// A plugin that sets up the server by adding the [`ServerConfig`] resource and the [`SharedPlugin`] plugin.
struct SetupPlugin {
    config: ServerConfig,
}

impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        app
            // RESOURCES //
            .insert_resource(self.config.clone());
        // PLUGINS
        // NOTE: SharedPlugin needs to be added after config
        if !app.is_plugin_added::<SharedPlugin>() {
            app.add_plugins(SharedPlugin {
                config: self.config.shared,
            });
        }
    }
}
