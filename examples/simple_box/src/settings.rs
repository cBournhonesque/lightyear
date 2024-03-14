use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport {
        certificate_digest: String,
    },
    WebSocket,
    Steam {
        app_id: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ServerTransports {
    Udp {
        local_port: u16,
    },
    WebTransport {
        local_port: u16,
    },
    WebSocket {
        local_port: u16,
    },
    Steam {
        app_id: u32,
        server_ip: Ipv4Addr,
        game_port: u16,
        query_port: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Conditioner {
    /// One way latency in milliseconds
    pub(crate) latency_ms: u16,
    /// One way jitter in milliseconds
    pub(crate) jitter_ms: u16,
    /// Percentage of packet loss
    pub(crate) packet_loss: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerSettings {
    /// If true, disable any rendering-related plugins
    pub(crate) headless: bool,

    /// If true, enable bevy_inspector_egui
    pub(crate) inspector: bool,

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,

    /// Which transport to use
    pub(crate) transport: Vec<ServerTransports>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientSettings {
    /// If true, enable bevy_inspector_egui
    pub(crate) inspector: bool,

    /// The client id
    pub(crate) client_id: u64,

    /// The client port to listen on
    pub(crate) client_port: u16,

    /// The ip address of the server
    pub(crate) server_addr: Ipv4Addr,

    /// The port of the server
    pub(crate) server_port: u16,

    /// Which transport to use
    pub(crate) transport: ClientTransports,

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
pub struct SharedSettings {
    /// An id to identify the protocol version
    pub(crate) protocol_id: u64,

    /// a 32-byte array to authenticate via the Netcode.io protocol
    pub(crate) private_key: [u8; 32],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub client: ClientSettings,
    pub shared: SharedSettings,
}
