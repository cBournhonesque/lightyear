//! This module contains the shared code between the client and the server.

use bevy::prelude::*;
use core::time::Duration;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use lightyear::prelude::*;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

pub const SERVER_REPLICATION_INTERVAL: Duration = Duration::from_millis(100);

pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

/// The [`SharedConfig`] must be shared between the `ClientConfig` and `ServerConfig`
pub fn shared_config() -> SharedConfig {
    SharedConfig {
        // send an update every 100ms
        client_replication_send_interval: SERVER_REPLICATION_INTERVAL,
        server_replication_send_interval: SERVER_REPLICATION_INTERVAL,
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        },
    }
}

#[derive(Clone)]
pub struct SharedPlugin;

#[derive(Channel)]
pub struct Channel1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Register your protocol, which is shared between client and server
        app.register_message::<Message1>(ChannelDirection::Bidirectional);
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
