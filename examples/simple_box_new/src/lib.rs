use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear_examples_common_new::shared::SharedSettings;

pub mod protocol;

pub mod client;

pub mod server;

pub mod renderer;

pub mod shared;

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
pub const SERVER_PORT: u16 = 5000;
/// 0 means that the OS will assign any available port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SERVER_PORT);
pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0,
    ],
};

pub const SEND_INTERVAL: Duration = Duration::from_millis(100);