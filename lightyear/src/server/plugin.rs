//! Defines the server bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use crate::prelude::MessageRegistry;
use bevy::prelude::*;

use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::connection::ConnectionManager;
use crate::server::events::ServerEventsPlugin;
use crate::server::input::InputPlugin;
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
            .init_resource::<MessageRegistry>()
            // PLUGINS
            .add_plugins(ServerEventsPlugin::default())
            .add_plugins(ServerNetworkingPlugin::default())
            .add_plugins(RoomPlugin::default())
            .add_plugins(ServerReplicationPlugin::default())
            .add_plugins(SharedPlugin {
                // TODO: move shared config out of server_config?
                config: self.config.shared.clone(),
                ..default()
            });
    }
}
