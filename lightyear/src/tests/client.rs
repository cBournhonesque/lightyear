use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use crate::connection::client::NetConfig;
use bevy::app::App;

use crate::prelude::client::*;
use crate::prelude::*;
use crate::tests::protocol::*;

pub fn bevy_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let io = Io::from_config(IoConfig::from_transport(TransportConfig::UdpSocket(addr)));
    let config = ClientConfig {
        shared: SharedConfig {
            enable_replication: false,
            tick: TickConfig::new(Duration::from_millis(10)),
            ..Default::default()
        },
        input: InputConfig::default(),
        net: NetConfig::Netcode {
            auth,
            config: Default::default(),
            io,
        },
        ping: PingConfig::default(),
        sync: SyncConfig::default(),
        prediction: PredictionConfig::default(),
        interpolation: InterpolationConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol());
    let plugin = ClientPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
