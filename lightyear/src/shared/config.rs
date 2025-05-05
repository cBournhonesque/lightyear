//! Configuration that has to be the same between the server and the client.
use bevy::reflect::Reflect;
use core::time::Duration;

use crate::shared::tick_manager::TickConfig;

/// Configuration that has to be the same between the server and the client.
#[derive(Clone, Copy, Debug, Reflect)]
pub struct SharedConfig {
    /// How often does the server send replication updates to the client?
    /// A duration of 0 means that we send replication updates every frame.
    ///
    /// This setting is present here and not in the server's ReplicationConfig
    /// because the client needs to have access to this value to compute
    /// how much interpolation delay to use.
    pub server_replication_send_interval: Duration,
    /// How often does the client send replication updates to the server?
    /// A duration of 0 means that we send replication updates every frame.
    pub client_replication_send_interval: Duration,
    /// configuration for the [`FixedUpdate`](bevy::prelude::FixedUpdate) schedule
    pub tick: TickConfig,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            server_replication_send_interval: Duration::from_millis(0),
            client_replication_send_interval: Duration::from_millis(0),
            tick: TickConfig::new(Duration::from_millis(16)),
        }
    }
}
