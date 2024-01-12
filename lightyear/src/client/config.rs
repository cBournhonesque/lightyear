//! Defines client-specific configuration options
use bevy::utils::Duration;

use crate::client::input::InputConfig;
use crate::client::interpolation::plugin::InterpolationConfig;
use crate::client::prediction::plugin::PredictionConfig;
use crate::client::sync::SyncConfig;
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
            client_timeout_secs: 10,
        }
    }
}

impl NetcodeConfig {
    pub(crate) fn build(&self) -> crate::netcode::ClientConfig<()> {
        crate::netcode::ClientConfig::default()
            .num_disconnect_packets(self.num_disconnect_packets)
            .packet_send_rate(self.keepalive_packet_send_rate)
    }
}

#[derive(Clone)]
pub struct PacketConfig {
    /// how often do we send packets to the server?
    /// (the minimum is once per frame)
    pub(crate) packet_send_interval: Duration,
}

impl Default for PacketConfig {
    fn default() -> Self {
        Self {
            packet_send_interval: Duration::from_millis(100),
        }
    }
}

impl PacketConfig {
    pub fn with_packet_send_interval(mut self, packet_send_interval: Duration) -> Self {
        self.packet_send_interval = packet_send_interval;
        self
    }
}

#[derive(Clone, Default)]
pub struct ClientConfig {
    pub shared: SharedConfig,
    pub netcode: NetcodeConfig,
    pub input: InputConfig,
    pub ping: PingConfig,
    pub sync: SyncConfig,
    pub prediction: PredictionConfig,
    pub interpolation: InterpolationConfig,
}
