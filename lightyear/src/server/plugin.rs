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

pub struct PluginConfig<P: Protocol> {
    server_config: ServerConfig,
    protocol: P,
}

// TODO: put all this in ServerConfig?
impl<P: Protocol> PluginConfig<P> {
    pub fn new(server_config: ServerConfig, protocol: P) -> Self {
        PluginConfig {
            server_config,
            protocol,
        }
    }
}

pub struct ServerPlugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference
    // in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> ServerPlugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

impl<P: Protocol> Plugin for ServerPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();

        app
            // RESOURCES //
            .insert_resource(config.server_config.clone())
            .insert_resource(config.protocol.clone())
            .init_resource::<MessageRegistry>()
            // PLUGINS
            .add_plugins(ServerEventsPlugin::<P>::default())
            .add_plugins(ServerNetworkingPlugin::<P>::default())
            .add_plugins(RoomPlugin::<P>::default())
            // .add_plugins(ServerReplicationPlugin::<P>::default())
            .add_plugins(SharedPlugin::<P> {
                // TODO: move shared config out of server_config?
                config: config.server_config.shared.clone(),
                ..default()
            });
    }
}
