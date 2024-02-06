//! Defines client-specific configuration options
use bevy::prelude::Resource;
use governor::Quota;
use nonzero_ext::nonzero;

use crate::client::input::InputConfig;
use crate::client::interpolation::plugin::InterpolationConfig;
use crate::client::prediction::plugin::PredictionConfig;
use crate::client::sync::SyncConfig;
use crate::connection::client::NetConfig;
use crate::shared::config::SharedConfig;
use crate::shared::ping::manager::PingConfig;

#[derive(Clone)]
/// Config related to the netcode protocol (abstraction of a connection over raw UDP-like transport)
pub struct NetcodeConfig {
    pub num_disconnect_packets: usize,
    pub keepalive_packet_send_rate: f64,
    /// if we don't hear from the client for this duration, we disconnect them
    /// A negative value means no timeout.
    /// This is used for Authentication::Manual tokens
    pub client_timeout_secs: i32,
}

impl Default for NetcodeConfig {
    fn default() -> Self {
        Self {
            num_disconnect_packets: 10,
            keepalive_packet_send_rate: 1.0 / 10.0,
            client_timeout_secs: 3,
        }
    }
}

impl NetcodeConfig {
    pub(crate) fn build(&self) -> crate::connection::netcode::ClientConfig<()> {
        crate::connection::netcode::ClientConfig::default()
            .num_disconnect_packets(self.num_disconnect_packets)
            .packet_send_rate(self.keepalive_packet_send_rate)
    }
}

#[derive(Clone)]
pub struct PacketConfig {
    /// Number of bytes per second that can be sent to the server
    pub send_bandwidth_cap: Quota,
    /// If false, there is no bandwidth cap and all messages are sent as soon as possible
    pub bandwidth_cap_enabled: bool,
}

impl Default for PacketConfig {
    fn default() -> Self {
        Self {
            // 56 KB/s bandwidth cap
            send_bandwidth_cap: Quota::per_second(nonzero!(56000u32)),
            bandwidth_cap_enabled: false,
        }
    }
}

impl PacketConfig {
    pub fn with_send_bandwidth_cap(mut self, send_bandwidth_cap: Quota) -> Self {
        self.send_bandwidth_cap = send_bandwidth_cap;
        self
    }

    pub fn with_send_bandwidth_bytes_per_second_cap(mut self, send_bandwidth_cap: u32) -> Self {
        let cap = send_bandwidth_cap.try_into().unwrap();
        self.send_bandwidth_cap = Quota::per_second(cap).allow_burst(cap);
        self
    }

    pub fn enable_bandwidth_cap(mut self) -> Self {
        self.bandwidth_cap_enabled = true;
        self
    }
}

/// The configuration object that lets you create a `ClientPlugin` with the desired settings.
///
/// Most of the fields are optional and have sensible defaults.
/// You do need to provide a [`SharedConfig`] struct that has to be same on the client and the server.
/// ```rust, ignore
/// let config = ClientConfig {
///    shared: SharedConfig::default(),
///    ..default()
/// };
/// let client = ClientPlugin::new(PluginConfig::new(config, io, MyProtocol::default()));
/// ```
#[derive(Resource, Clone, Default)]
pub struct ClientConfig {
    pub shared: SharedConfig,
    pub packet: PacketConfig,
    pub net: NetConfig,
    pub input: InputConfig,
    pub ping: PingConfig,
    pub sync: SyncConfig,
    pub prediction: PredictionConfig,
    pub interpolation: InterpolationConfig,
}
