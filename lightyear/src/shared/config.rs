//! Configuration that has to be the same between the server and the client.
use bevy::utils::Duration;

use crate::shared::tick_manager::TickConfig;

/// Configuration that has to be the same between the server and the client.
#[derive(Clone, Debug)]
pub struct SharedConfig {
    /// how often does the client send updates to the server?
    pub client_send_interval: Duration,
    /// how often does the server send updates to the client?
    pub server_send_interval: Duration,
    /// configuration for the [`FixedUpdate`](bevy::prelude::FixedUpdate) schedule
    pub tick: TickConfig,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            // 0 means that we send updates every frame
            client_send_interval: Duration::from_millis(0),
            server_send_interval: Duration::from_millis(0),
            tick: TickConfig::new(Duration::from_millis(16)),
        }
    }
}
