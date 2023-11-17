use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;

use lightyear_shared::client::{
    Authentication, Client, ClientConfig, PingConfig, Plugin, PluginConfig,
};
use lightyear_shared::{IoConfig, SharedConfig, TickConfig, TransportConfig};

use crate::protocol::{protocol, MyProtocol};

pub fn setup(auth: Authentication) -> anyhow::Result<Client<MyProtocol>> {
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let config = ClientConfig {
        shared: SharedConfig {
            enable_replication: false,
            tick: TickConfig::new(Duration::from_millis(10)),
            ..Default::default()
        },
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
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
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol(), auth);
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(plugin);
}
