use crate::client::sync::SyncConfig;
use crate::{IoConfig, SharedConfig};

use super::ping_manager::PingConfig;

#[derive(Clone)]
pub struct NetcodeConfig {
    pub num_disconnect_packets: usize,
    pub packet_send_rate: f64,
}

impl Default for NetcodeConfig {
    fn default() -> Self {
        Self {
            num_disconnect_packets: 10,
            packet_send_rate: 1.0 / 10.0,
        }
    }
}

impl NetcodeConfig {
    pub(crate) fn build(&self) -> crate::netcode::ClientConfig<()> {
        crate::netcode::ClientConfig::default()
            .num_disconnect_packets(self.num_disconnect_packets)
            .packet_send_rate(self.packet_send_rate)
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    pub shared: SharedConfig,
    pub netcode: NetcodeConfig,
    pub io: IoConfig,
    pub ping: PingConfig,
    pub sync: SyncConfig,
}
