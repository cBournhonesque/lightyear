use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;

use lightyear_shared::prelude::client::*;
use lightyear_shared::prelude::*;

use crate::protocol::{protocol, MyProtocol};

pub fn setup(auth: Authentication) -> anyhow::Result<Client<MyProtocol>> {
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let config = ClientConfig {
        shared: SharedConfig {
            enable_replication: false,
            tick: TickConfig::new(Duration::from_millis(10)),
            ..Default::default()
        },
        input: InputConfig::default(),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
        sync: SyncConfig::default(),
        prediction: PredictionConfig::default(),
        interpolation: InterpolationConfig::default(),
    };

    // create lightyear client
    Ok(Client::new(config, auth, protocol()))
}

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
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
        sync: SyncConfig::default(),
        prediction: PredictionConfig::default(),
        interpolation: InterpolationConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol(), auth);
    let plugin = ClientPlugin::new(plugin_config);
    app.add_plugins(plugin);
}
