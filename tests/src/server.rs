use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::App;
use bevy::log::LogPlugin;
use bevy::prelude::PluginGroup;
use bevy::DefaultPlugins;
use tracing::Level;

use lightyear_server::PluginConfig;
use lightyear_server::{NetcodeConfig, Plugin};
use lightyear_server::{Server, ServerConfig};
use lightyear_shared::netcode::{ClientId, ConnectToken};
use lightyear_shared::IoConfig;

use crate::protocol::{protocol, MyProtocol};

pub fn setup() -> anyhow::Result<Server<MyProtocol>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let config = ServerConfig {
        netcode: NetcodeConfig::default(),
        io: IoConfig::UdpSocket(addr),
    };

    // create lightyear server
    Ok(Server::new(config, 0, protocol()))
}

pub fn bevy_setup(app: &mut App, client_id: ClientId) -> ConnectToken {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ServerConfig {
        netcode: NetcodeConfig::default(),
        io: IoConfig::UdpSocket(addr),
    };
    let plugin_config = PluginConfig::new(config, 0, protocol());
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(DefaultPlugins.set(LogPlugin {
        level: Level::TRACE,
        filter: "lightyear=trace,lightyear_server=trace,lightyear_tests=trace".to_string(),
    }))
    .add_plugins(plugin);

    app.world
        .resource_mut::<Server<MyProtocol>>()
        .token(client_id)
}
