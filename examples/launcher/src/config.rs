// src/config.rs
use bevy::prelude::{Reflect, Resource};
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
// Import serde traits
use lightyear_examples_common::client::ClientTransports;
use lightyear_examples_common::server::ServerTransports;
use lightyear_examples_common::shared::SharedSettings;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

// --- Constants ---
pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
pub const SERVER_PORT: u16 = 5000;
/// 0 means that the OS will assign any available port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SERVER_PORT);
pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0,
    ],
};
pub const SEND_INTERVAL: Duration = Duration::from_millis(100);

// --- Configuration Enums ---

// TODO: Discover examples dynamically? For now, hardcode them.
// Make sure these derive Serialize and Deserialize
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display, Serialize, Deserialize, Reflect,
)]
pub enum Example {
    SimpleBox,
    Fps,
    // Add other examples here
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    Display,
    EnumString,
    Serialize,
    Deserialize,
    Reflect,
)]
pub enum NetworkingMode {
    ClientOnly,
    ServerOnly,
    HostServer, // Server + Client in the same app
}

// --- Main Configuration Struct ---

// Add Serialize, Deserialize derives
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct LauncherConfig {
    pub example: Example,
    pub mode: NetworkingMode,
    // Use Options for conditional settings
    pub client_transport: Option<ClientTransports>,
    pub server_transport: Option<ServerTransports>,
    pub client_id: Option<u64>,
    pub server_addr: Option<SocketAddr>, // Used for client connect AND server bind
    #[serde(with = "duration_serde")] // Use helper for Duration serialization
    pub tick_duration: Duration,
    // TODO: Add LinkConditioner settings
    // TODO: Add other settings like auth, encryption?
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let default_mode = NetworkingMode::HostServer;
        Self {
            example: Example::SimpleBox,
            mode: default_mode,
            // Set initial defaults based on HostServer mode
            client_transport: Some(ClientTransports::Udp),
            client_id: Some(0),
            server_addr: Some(SERVER_ADDR),
            server_transport: Some(ServerTransports::Udp {
                local_port: SERVER_PORT,
            }),
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        }
    }
}

// Helper module for Duration serialization/deserialization
mod duration_serde {
    use core::time::Duration;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(duration.as_secs_f64())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = f64::deserialize(deserializer)?;
        Ok(Duration::from_secs_f64(secs))
    }
}

// Need FromStr for Example to be used in UI Combobox or initial default parsing if needed elsewhere
// Note: Clap will no longer use this directly.
impl std::str::FromStr for Example {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "simplebox" => Ok(Example::SimpleBox),
            "fps" => Ok(Example::Fps),
            _ => Err(format!("Unknown example: {}", s)),
        }
    }
}

impl LauncherConfig {
    pub fn update_defaults_for_mode(&mut self, new_mode: NetworkingMode) {
        self.mode = new_mode;
        match new_mode {
            NetworkingMode::ClientOnly => {
                if self.client_id.is_none() {
                    self.client_id = Some(rand::random());
                }
                if self.server_addr.is_none() {
                    self.server_addr = Some(SERVER_ADDR);
                }
                if self.client_transport.is_none() {
                    self.client_transport = Some(ClientTransports::Udp);
                }
                self.server_transport = None;
            }
            NetworkingMode::ServerOnly => {
                if self.server_addr.is_none() {
                    self.server_addr = Some(SERVER_ADDR);
                }
                if self.server_transport.is_none() {
                    self.server_transport = Some(ServerTransports::Udp {
                        local_port: self.server_addr.map_or(SERVER_PORT, |a| a.port()),
                    });
                }
                self.client_id = None;
                self.client_transport = None;
            }
            NetworkingMode::HostServer => {
                if self.client_id.is_none() {
                    self.client_id = Some(rand::random());
                }
                if self.server_addr.is_none() {
                    self.server_addr = Some(SERVER_ADDR);
                }
                if self.client_transport.is_none() {
                    self.client_transport = Some(ClientTransports::Udp);
                }
                if self.server_transport.is_none() {
                    self.server_transport = Some(ServerTransports::Udp {
                        local_port: self.server_addr.map_or(SERVER_PORT, |a| a.port()),
                    });
                }
            }
        }
    }
}
