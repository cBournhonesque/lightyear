use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::{App, PluginGroup};
use bevy::log::{Level, LogPlugin};
use bevy::DefaultPlugins;

use lightyear_client::{ClientConfig, Plugin, PluginConfig};
use lightyear_shared::netcode::ConnectToken;
use lightyear_shared::IoConfig;

use crate::protocol::{protocol, MyProtocol};

pub fn setup(token: ConnectToken) -> anyhow::Result<lightyear_client::Client<MyProtocol>> {
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let config = ClientConfig {
        netcode: Default::default(),
        io: IoConfig::UdpSocket(addr),
    };

    // create lightyear client
    Ok(lightyear_client::Client::new(config, token, protocol()))
}

pub fn bevy_setup(app: &mut App, token: ConnectToken) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ClientConfig {
        netcode: Default::default(),
        io: IoConfig::UdpSocket(addr),
    };
    let plugin_config = PluginConfig::new(config, protocol(), token);
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(DefaultPlugins.set(LogPlugin {
        level: Level::TRACE,
        filter: "lightyear=trace,lightyear_client=trace,lightyear_tests=trace".to_string(),
    }))
    .add_plugins(plugin);
}
