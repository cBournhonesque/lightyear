use std::net::SocketAddr;
use std::str::FromStr;

use anyhow::Result;
use bevy::prelude::Resource;

use crate::_reexport::ReadWordBuffer;
use crate::client::config::NetcodeConfig;
use crate::connection::netcode::{ClientId, ConnectToken};

#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::connection::steam::client::SteamConfig;
use crate::packet::packet::Packet;

use crate::prelude::{generate_key, Io, IoConfig, Key, LinkConditionerConfig};

// TODO: add diagnostics methods?
pub trait NetClient: Send + Sync {
    // type Error;

    /// Connect to server
    fn connect(&mut self) -> Result<()>;

    /// Returns true if the client is connected to the server
    fn is_connected(&self) -> bool;

    /// Update the connection state + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<()>;

    /// Receive a packet from the server
    fn recv(&mut self) -> Option<Packet>;

    /// Send a packet to the server
    fn send(&mut self, buf: &[u8]) -> Result<()>;

    /// Get the id of the client
    fn id(&self) -> ClientId;

    /// Get the local address of the client
    fn local_addr(&self) -> SocketAddr;

    /// Get immutable access to the inner io
    fn io(&self) -> Option<&Io>;

    /// Get mutable access to the inner io
    fn io_mut(&mut self) -> Option<&mut Io>;
}

/// Resource that holds the client connection
#[derive(Resource)]
pub struct ClientConnection {
    pub(crate) client: Box<dyn NetClient>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum NetConfig {
    Netcode {
        auth: Authentication,
        config: NetcodeConfig,
        io: IoConfig,
    },
    // TODO: for steam, we can use a pass-through io that just computes stats?
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam {
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    },
}

impl Default for NetConfig {
    fn default() -> Self {
        Self::Netcode {
            auth: Authentication::default(),
            config: NetcodeConfig::default(),
            io: IoConfig::default(),
        }
    }
}

impl NetConfig {
    pub fn build_client(self) -> ClientConnection {
        match self {
            NetConfig::Netcode {
                auth,
                config,
                io: io_config,
            } => {
                let token = auth
                    .clone()
                    .get_token(config.client_timeout_secs)
                    .expect("could not generate token");
                let token_bytes = token.try_into_bytes().unwrap();
                let netcode =
                    super::netcode::NetcodeClient::with_config(&token_bytes, config.build())
                        .expect("could not create netcode client");
                let client = super::netcode::Client {
                    client: netcode,
                    io_config,
                    io: None,
                };
                ClientConnection {
                    client: Box::new(client),
                }
            }
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam {
                config,
                conditioner,
            } => {
                // TODO: handle errors
                let client = super::steam::client::Client::new(config, conditioner)
                    .expect("could not create steam client");
                ClientConnection {
                    client: Box::new(client),
                }
            }
        }
    }
}

impl NetClient for ClientConnection {
    fn connect(&mut self) -> Result<()> {
        self.client.connect()
    }

    fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        self.client.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<Packet> {
        self.client.recv()
    }

    fn send(&mut self, buf: &[u8]) -> Result<()> {
        self.client.send(buf)
    }

    fn id(&self) -> ClientId {
        self.client.id()
    }

    fn local_addr(&self) -> SocketAddr {
        self.client.local_addr()
    }

    fn io(&self) -> Option<&Io> {
        self.client.io()
    }

    fn io_mut(&mut self) -> Option<&mut Io> {
        self.client.io_mut()
    }
}

#[derive(Resource, Default, Clone)]
#[allow(clippy::large_enum_variant)]
/// Struct used to authenticate with the server
pub enum Authentication {
    /// Use a `ConnectToken` that was already received (usually from a secure-connection to a webserver)
    Token(ConnectToken),
    /// Or build a `ConnectToken` manually from the given parameters
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
    #[default]
    /// Request a connect token from the backend
    RequestConnectToken,
}

impl Authentication {
    pub fn get_token(self, client_timeout_secs: i32) -> Option<ConnectToken> {
        match self {
            Authentication::Token(token) => Some(token),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .timeout_seconds(client_timeout_secs)
                .generate()
                .ok(),
            Authentication::RequestConnectToken => {
                // create a fake connect token so that we have a NetcodeClient
                ConnectToken::build(
                    SocketAddr::from_str("0.0.0.0:0").unwrap(),
                    0,
                    0,
                    generate_key(),
                )
                .timeout_seconds(client_timeout_secs)
                .generate()
                .ok()
            }
        }
    }
}
