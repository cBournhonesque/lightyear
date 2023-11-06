use std::default::Default;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;

use lightyear_server::PluginConfig;
use lightyear_server::{NetcodeConfig, Plugin};
use lightyear_server::{Server, ServerConfig};
use lightyear_shared::netcode::Key;
use lightyear_shared::{IoConfig, TickConfig, TransportConfig};

use crate::protocol::{protocol, MyProtocol};

pub fn setup(protocol_id: u64, private_key: Key) -> anyhow::Result<Server<MyProtocol>> {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let netcode_config = NetcodeConfig::default()
        .with_protocol_id(protocol_id)
        .with_key(private_key);
    let config = ServerConfig {
        netcode: netcode_config,
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        tick: TickConfig::new(Duration::from_millis(16)),
    };

    // create lightyear server
    Ok(Server::new(config, protocol()))
}

pub fn bevy_setup(app: &mut App, addr: SocketAddr, protocol_id: u64, private_key: Key) {
    // create udp-socket based io
    let netcode_config = NetcodeConfig::default()
        .with_protocol_id(protocol_id)
        .with_key(private_key);
    let config = ServerConfig {
        netcode: netcode_config,
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        tick: TickConfig::new(Duration::from_millis(16)),
    };
    let plugin_config = PluginConfig::new(config, protocol());
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(plugin);
}
