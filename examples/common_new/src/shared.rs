use core::time::Duration;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
pub const REPLICATION_INTERVAL: Duration = Duration::from_millis(100);

// /// The [`SharedConfig`] must be shared between the `ClientConfig` and `ServerConfig`
// pub fn shared_config() -> SharedConfig {
//     SharedConfig {
//         // send replication updates every 100ms
//         server_replication_send_interval: REPLICATION_INTERVAL,
//         client_replication_send_interval: REPLICATION_INTERVAL,
//         tick: TickConfig {
//             tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
//         },
//     }
// }


#[derive(Copy, Clone, Debug)]
pub struct SharedSettings {
    /// An id to identify the protocol version
    pub protocol_id: u64,

    /// a 32-byte array to authenticate via the Netcode.io protocol
    pub private_key: [u8; 32],
}
