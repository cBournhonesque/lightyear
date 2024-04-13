//! Configuration that has to be the same between the server and the client.
use bevy::prelude::Res;
use bevy::reflect::Reflect;
use bevy::utils::Duration;

use crate::server::config::ServerConfig;
use crate::shared::tick_manager::TickConfig;

/// Configuration that has to be the same between the server and the client.
#[derive(Clone, Debug, Reflect)]
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

// TODO: maybe the modes should just be
//  - server and client are running in separate apps: need to add SharedPlugin on client, etc.
//  - server and client are running in same app: need to add SharedPlugin on client, need to only add LeafwingInputOnce
//    - host-server mode activated <> we use LocalTransport on client, server runs some connections <> disable all prediction, etc. on client
//    - host-server mode non-activate <> we use a non-Local transport on client, server has no connections <> still all run all prediction, networking on client; disable server entirely

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum Mode {
    #[default]
    /// Run the client and server in two different apps
    Separate,
    /// Run only the server, but can support a local player
    HostServer,
    // /// We will run both the client and server plugins in the same app, but all server plugins are disabled.
    // /// This is useful so that we can switch at runtime between separate and host-server mode
    // ClientOnly,
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
