use lightyear::prelude::{Mode, SharedConfig, TickConfig};
use std::time::Duration;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

pub const SERVER_REPLICATION_INTERVAL: Duration = Duration::from_millis(500);

/// The [`SharedConfig`] must be shared between the `ClientConfig` and `ServerConfig`
pub fn shared_config(mode: Mode) -> SharedConfig {
    SharedConfig {
        // send replication updates every 100ms
        server_replication_send_interval: SERVER_REPLICATION_INTERVAL,
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        },
        mode,
    }
}
