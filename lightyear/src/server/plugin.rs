//! Defines the server bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::*;
use tracing::error;

use crate::connection::server::NetServer;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::connection::ConnectionManager;
use crate::server::events::{ConnectEvent, ServerEventsPlugin};
use crate::server::input::InputPlugin;
use crate::server::networking::ServerNetworkingPlugin;
use crate::server::replication::ServerReplicationPlugin;
use crate::server::room::RoomPlugin;
use crate::shared::plugin::SharedPlugin;
use crate::shared::replication::plugin::ReplicationPlugin;
use crate::shared::time_manager::TimePlugin;

use super::config::ServerConfig;

pub struct PluginConfig<P: Protocol> {
    server_config: ServerConfig,
    protocol: P,
}

// TODO: put all this in ClientConfig?
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
        let tick_duration = config.server_config.shared.tick.tick_duration;

        app
            // RESOURCES //
            .insert_resource(config.server_config.clone())
            .insert_resource(ConnectionManager::<P>::new(
                config.protocol.channel_registry().clone(),
                config.server_config.packet,
                config.server_config.ping,
            ))
            // PLUGINS
            .add_plugins(ServerEventsPlugin::<P>::default())
            .add_plugins(ServerNetworkingPlugin::<P>::new(config.server_config.net))
            .add_plugins(InputPlugin::<P>::default())
            .add_plugins(RoomPlugin::<P>::default())
            .add_plugins(ServerReplicationPlugin::<P>::default())
            .add_plugins(SharedPlugin::<P> {
                // TODO: move shared config out of server_config?
                config: config.server_config.shared.clone(),
                ..default()
            });
    }
}
