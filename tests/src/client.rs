use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::app::App;

use lightyear_shared::client::config::PacketConfig;
use lightyear_shared::client::interpolation::plugin::InterpolationConfig;
use lightyear_shared::client::prediction::plugin::PredictionConfig;
use lightyear_shared::client::{
    Authentication, Client, ClientConfig, PingConfig, Plugin, PluginConfig, SyncConfig,
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
        packet: PacketConfig::default().with_packet_send_interval(Duration::from_millis(0)),
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
        packet: PacketConfig::default().with_packet_send_interval(Duration::from_millis(0)),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
        sync: SyncConfig::default(),
        prediction: PredictionConfig::default(),
        interpolation: InterpolationConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol(), auth);
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(plugin);
}
