//! Configuration that has to be the same between the server and the client.
use bevy::reflect::Reflect;
use bevy::utils::Duration;

use crate::shared::tick_manager::TickConfig;

/// Configuration that has to be the same between the server and the client.
#[derive(Clone, Copy, Debug, Reflect)]
pub struct SharedConfig {
    /// how often does the server send replication updates to the client?
    /// A duration of 0 means that we send replication updates every frame
    pub server_replication_send_interval: Duration,
    /// configuration for the [`FixedUpdate`](bevy::prelude::FixedUpdate) schedule
    pub tick: TickConfig,
    pub mode: Mode,
}

// TODO: maybe the modes should just be
//  - server and client are running in separate apps: need to add SharedPlugin on client, etc.
//  - server and client are running in same app: need to add SharedPlugin on client, need to only add LeafwingInputOnce
//    - host-server mode activated <> we use LocalTransport on client, server runs some connections <> disable all prediction, etc. on client
//    - host-server mode non-activate <> we use a non-Local transport on client, server has no connections <> still run all prediction, networking on client; disable server entirely

// TODO: maybe we can figure the mode out directly from the registered plugins and the networking state instead of requiring
//  the user to specify the mode.
//  - If we see only the client plugin or the server plugin, we are in Separate mode
//  - If we see both plugins and we use LocalTransport on Client and the server is started, we are in HostServer mode
//  - If we see both plugins and we use a non-local transport on client or the server is not started, we are in ClientOnly mode?
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum Mode {
    #[default]
    /// Run the client and server in two different apps
    Separate,
    /// Run only the server, but can support a local player
    /// This means that the ServerPlugin and ClientPlugin are running in the same App.
    HostServer,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            server_replication_send_interval: Duration::from_millis(0),
            tick: TickConfig::new(Duration::from_millis(16)),
            mode: Mode::default(),
        }
    }
}
