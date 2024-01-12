use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;
use bevy::prelude::default;

use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::{protocol, MyProtocol};

pub fn bevy_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let io = Io::from_config(IoConfig::from_transport(TransportConfig::UdpSocket(addr)));
    let config = ClientConfig {
        shared: SharedConfig {
            tick: TickConfig::new(Duration::from_millis(10)),
            ..default()
        },
        ..default()
    };
    let plugin_config = PluginConfig::new(config, io, protocol(), auth);
    let plugin = ClientPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
