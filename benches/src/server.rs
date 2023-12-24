use std::default::Default;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;
use bevy::prelude::default;

use crate::protocol::{protocol, MyProtocol};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub fn bevy_setup(app: &mut App, addr: SocketAddr, protocol_id: u64, private_key: Key) {
    // create udp-socket based io
    let io = Io::from_config(&IoConfig::from_transport(TransportConfig::UdpSocket(addr)));
    let config = ServerConfig {
        shared: SharedConfig {
            tick: TickConfig::new(Duration::from_millis(10)),
            ..default()
        },
        netcode: NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key),
        ..default()
    };
    let plugin_config = PluginConfig::new(config, io, protocol());
    let plugin = ServerPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
