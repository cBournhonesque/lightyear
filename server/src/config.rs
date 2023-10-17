use lightyear_shared::netcode::Key;
use lightyear_shared::IoConfig;

pub struct NetcodeConfig {
    pub num_disconnect_packets: usize,
    pub keep_alive_send_rate: f64,
    pub private_key: Option<Key>,
}

impl Default for NetcodeConfig {
    fn default() -> Self {
        Self {
            num_disconnect_packets: 10,
            keep_alive_send_rate: 1.0 / 10.0,
            private_key: None,
        }
    }
}

impl NetcodeConfig {
    fn with_key(mut self, key: Key) -> Self {
        self.private_key = Some(key);
        self
    }
}

pub struct ServerConfig {
    pub netcode: NetcodeConfig,
    pub io: IoConfig,
}
