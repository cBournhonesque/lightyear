use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::App;
use bevy::prelude::default;

use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;

pub fn bevy_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ClientConfig {
        shared: SharedConfig {
            enable_replication: false,
            tick: TickConfig::new(Duration::from_millis(10)),
            ..Default::default()
        },
        input: InputConfig::default(),
        net: NetConfig::Netcode {
            config: NetcodeConfig::default(),
            auth,
            io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        },
        ping: PingConfig::default(),
        sync: SyncConfig::default(),
        prediction: PredictionConfig::default(),
        interpolation: InterpolationConfig::default(),
        ..default()
    };
    let plugin_config = PluginConfig::new(config, protocol());
    let plugin = ClientPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
