use bevy::prelude::{Component, Fixed, Time};
use lightyear_client::{Authentication, ClientConfig, PingConfig, Plugin, PluginConfig};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::{component_protocol, message_protocol};
use lightyear_shared::{protocolize, Channel, Message};
use lightyear_shared::{App, IoConfig, Protocol, SharedConfig, TickConfig, TransportConfig};
use lightyear_shared::{ChannelDirection, ChannelMode, ChannelSettings};

// Messages
#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Message, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

#[derive(Debug, PartialEq)]
#[message_protocol(protocol = "MyProtocol")]
pub enum MyMessageProtocol {
    Message1(Message1),
    Message2(Message2),
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component1;

#[derive(Debug, PartialEq)]
#[component_protocol(protocol = "MyProtocol")]
pub enum MyComponentsProtocol {
    Component1(Component1),
}

protocolize!(MyProtocol, MyMessageProtocol, MyComponentsProtocol);

// Channels
#[derive(Channel)]
pub struct Channel1;

#[derive(Channel)]
pub struct Channel2;

pub fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        direction: ChannelDirection::Bidirectional,
    });
    p.add_channel::<Channel2>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        direction: ChannelDirection::Bidirectional,
    });
    p
}
pub fn app_setup(app: &mut App, auth: Authentication) {
    // create udp-socket based io
    let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
    let config = ClientConfig {
        shared: SharedConfig::default(),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
        tick: TickConfig::new(Duration::from_millis(10)),
        ping: PingConfig::default(),
    };
    let plugin_config = PluginConfig::new(config, protocol(), auth);
    let plugin = Plugin::new(plugin_config);
    app.add_plugins(plugin);
}

// Simulate that our fixed timestep has elapsed
// and do 1 app.update
pub fn tick(app: &mut App) {
    let mut fxt = app.world.resource_mut::<Time<Fixed>>();
    let timestep = fxt.timestep();
    fxt.advance_by(fxt.timestep());
    app.update();
}
