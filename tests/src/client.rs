use std::net::SocketAddr;
use std::str::FromStr;

use bevy::app::App;

use lightyear_client::{Authentication, ClientConfig, PingConfig, Plugin, PluginConfig};
use lightyear_shared::{IoConfig, SharedConfig, TransportConfig};

use crate::protocol::{protocol, MyProtocol};

pub fn setup(auth: Authentication) -> anyhow::Result<lightyear_client::Client<MyProtocol>> {
    let addr = SocketAddr::from_str("127.0.0.1:0")?;
    let config = ClientConfig {
        shared: SharedConfig::default(),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
    };

    // create lightyear client
    Ok(lightyear_client::Client::new(config, auth, protocol()))
}

pub fn bevy_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ClientConfig {
        shared: SharedConfig::default(),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        ping: PingConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol(), auth);
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(plugin);
}
