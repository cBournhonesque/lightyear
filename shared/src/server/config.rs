use crate::netcode::Key;
use crate::{IoConfig, SharedConfig};
use std::time::Duration;

use super::ping_manager::PingConfig;

#[derive(Clone)]
pub struct NetcodeConfig {
    pub num_disconnect_packets: usize,
    pub keep_alive_send_rate: f64,
    pub protocol_id: u64,
    pub private_key: Option<Key>,
}

impl Default for NetcodeConfig {
    fn default() -> Self {
        Self {
            num_disconnect_packets: 10,
            keep_alive_send_rate: 1.0 / 10.0,
            protocol_id: 0,
            private_key: None,
        }
    }
}

impl NetcodeConfig {
    pub fn with_protocol_id(mut self, protocol_id: u64) -> Self {
        self.protocol_id = protocol_id;
        self
    }
    pub fn with_key(mut self, key: Key) -> Self {
        self.private_key = Some(key);
        self
    }
}

#[derive(Clone)]
pub struct PacketConfig {
    /// how often do we send packets to the each client?
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

#[derive(Clone)]
pub struct ServerConfig {
    pub shared: SharedConfig,
    // TODO: maybe regroup packet/netcode/io into ConnectionConfig?
    pub packet: PacketConfig,
    pub netcode: NetcodeConfig,
    pub io: IoConfig,
    pub ping: PingConfig,
}
