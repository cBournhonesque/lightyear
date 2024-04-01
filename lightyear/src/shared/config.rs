//! Configuration that has to be the same between the server and the client.
use crate::prelude::ClientId;
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
    pub mode: Mode,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    #[default]
    /// Run the client and server in two different apps
    Separate,
    /// Run only the server, but can support a local player
    HostServer,
}

impl SharedConfig {
    pub fn is_host_server_condition(config: Option<Res<ServerConfig>>) -> bool {
        config.map_or(false, |config| {
            matches!(config.shared.mode, Mode::HostServer)
        })
    }
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            // 0 means that we send updates every frame
            client_send_interval: Duration::from_millis(0),
            server_send_interval: Duration::from_millis(0),
            tick: TickConfig::new(Duration::from_millis(16)),
            mode: Mode::default(),
        }
    }
}
