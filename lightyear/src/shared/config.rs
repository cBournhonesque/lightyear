//! Configuration that has to be the same between the server and the client.
use crate::prelude::{ClientId, UnifiedManager};
use crate::server::config::ServerConfig;
use bevy::prelude::Res;
use bevy::utils::Duration;

use crate::shared::tick_manager::TickConfig;

/// Configuration that has to be the same between the server and the client.
#[derive(Clone, Debug)]
pub struct SharedConfig {
    /// how often does the client send updates to the server?
    /// A duration of 0 means that we send updates every frame
    pub client_send_interval: Duration,
    /// how often does the server send updates to the client?
    /// A duration of 0 means that we send updates every frame
    pub server_send_interval: Duration,
    /// configuration for the [`FixedUpdate`](bevy::prelude::FixedUpdate) schedule
    pub tick: TickConfig,
    /// do we run in unified mode? (client and server in the same process)
    pub unified: bool,
}

pub const LOCAL_CLIENT_ID: ClientId = 0;

impl SharedConfig {
    /// Run condition that returns true if we are running in unified mode
    pub fn is_unified_condition(config: Res<ServerConfig>) -> bool {
        config.shared.unified
    }
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            // 0 means that we send updates every frame
            client_send_interval: Duration::from_millis(0),
            server_send_interval: Duration::from_millis(0),
            tick: TickConfig::new(Duration::from_millis(16)),
            unified: false,
        }
    }
}
