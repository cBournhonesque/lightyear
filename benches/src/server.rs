use bevy::utils::Duration;
use std::default::Default;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::App;
use bevy::prelude::default;

use crate::protocol::{protocol, MyProtocol};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub fn bevy_setup(app: &mut App, addr: SocketAddr, protocol_id: u64, private_key: Key) {
    // create udp-socket based io
    let config = ServerConfig {
        shared: SharedConfig {
            tick: TickConfig::new(Duration::from_millis(10)),
            ..default()
        },
        net: vec![NetConfig::Netcode {
            config: NetcodeConfig::default()
                .with_protocol_id(protocol_id)
                .with_key(private_key),
            io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        }],
        ..default()
    };
    let plugin_config = PluginConfig::new(config, protocol());
    let plugin = ServerPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
