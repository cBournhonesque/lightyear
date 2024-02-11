use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::App;
use bevy::prelude::default;

use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::{protocol, MyProtocol};

pub fn bevy_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ClientConfig {
        shared: SharedConfig {
            tick: TickConfig::new(Duration::from_millis(10)),
            ..default()
        },
        net: NetConfig::Netcode {
            auth,
            config: NetcodeConfig::default(),
            io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        },
        ..default()
    };
    let plugin_config = PluginConfig::new(config, protocol());
    let plugin = ClientPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
