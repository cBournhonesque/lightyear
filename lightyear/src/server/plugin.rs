//! Defines the server bevy plugin
use bevy::prelude::*;

use crate::server::events::ServerEventsPlugin;
use crate::server::networking::ServerNetworkingPlugin;
use crate::server::replication::ServerReplicationPlugin;
use crate::server::room::RoomPlugin;
use crate::shared::plugin::SharedPlugin;

use super::config::ServerConfig;

pub struct ServerPlugin {
    config: ServerConfig,
}

impl ServerPlugin {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app
            // RESOURCES //
            .insert_resource(self.config.clone())
            // PLUGINS
            // NOTE: SharedPlugin needs to be added after config
            .add_plugins(SharedPlugin {
                // TODO: move shared config out of server_config?
                config: self.config.shared.clone(),
            })
            .add_plugins(ServerEventsPlugin)
            .add_plugins(ServerNetworkingPlugin)
            .add_plugins(RoomPlugin)
            .add_plugins(ServerReplicationPlugin);
    }
}
