use std::net::SocketAddr;
use std::str::FromStr;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use std::sync::Arc;

use bevy::prelude::{Reflect, Resource};
use enum_dispatch::enum_dispatch;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use parking_lot::RwLock;

use crate::client::config::NetcodeConfig;
use crate::client::io::Io;
use crate::connection::id::ClientId;
use crate::connection::netcode::ConnectToken;

#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::connection::steam::{client::SteamConfig, steamworks_client::SteamworksClient};
use crate::packet::packet_builder::RecvPayload;

use crate::prelude::client::ClientTransport;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::prelude::LinkConditionerConfig;
use crate::prelude::{generate_key, Key};
use crate::transport::config::SharedIoConfig;

#[derive(Debug)]
pub enum ConnectionState {
    Disconnected { reason: Option<DisconnectReason> },
    Connecting,
    Connected,
}

// TODO: add diagnostics methods?
#[enum_dispatch]
pub trait NetClient: Send + Sync {
    /// Connect to server.
    ///
    /// Users should use [`ClientCommands`](crate::client::networking::ClientCommands) to initiate the connection process, as it
    /// also handles State transitions + additional stuff.
    fn connect(&mut self) -> Result<(), ConnectionError>;

    /// Disconnect from the server
    ///
    /// Users should use [`ClientCommands`](crate::client::networking::ClientCommands) to initiate the disconnection process, as it
    /// also handles State transitions + additional stuff.
    fn disconnect(&mut self) -> Result<(), ConnectionError>;

    /// Returns the [`ConnectionState`] of the client
    fn state(&self) -> ConnectionState;

    /// Update the connection state + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<(), ConnectionError>;

    /// Receive a packet from the server
    fn recv(&mut self) -> Option<RecvPayload>;

    /// Send a packet to the server
    fn send(&mut self, buf: &[u8]) -> Result<(), ConnectionError>;

    /// Get the id of the client
    fn id(&self) -> ClientId;

    /// Get the local address of the client
    fn local_addr(&self) -> SocketAddr;

    /// Get immutable access to the inner io
    fn io(&self) -> Option<&Io>;

    /// Get mutable access to the inner io
    fn io_mut(&mut self) -> Option<&mut Io>;
}

#[enum_dispatch(NetClient)]
pub enum NetClientDispatch {
    Netcode(super::netcode::Client<()>),
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam(super::steam::client::Client),
    Local(super::local::client::Client),
}

/// Resource that holds a [`NetClient`] instance.
/// (either a Netcode, Steam, or Local client)
#[derive(Resource)]
pub struct ClientConnection {
    pub client: NetClientDispatch,
    pub(crate) disconnect_reason: Option<DisconnectReason>,
}

/// Enumerates the possible reasons for a client to disconnect from the server
#[derive(Debug)]
pub enum DisconnectReason {
    Transport(crate::transport::error::Error),
    Netcode(super::netcode::ClientState),
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam(steamworks::networking_types::NetConnectionEnd),
}

pub type IoConfig = SharedIoConfig<ClientTransport>;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Reflect)]
#[reflect(from_reflect = false)]
pub enum NetConfig {
    Netcode {
        #[reflect(ignore)]
        auth: Authentication,
        config: NetcodeConfig,
        #[reflect(ignore)]
        io: IoConfig,
    },
    // TODO: for steam, we can use a pass-through io that just computes stats?
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam {
        #[reflect(ignore)]
        steamworks_client: Option<Arc<RwLock<SteamworksClient>>>,
        #[reflect(ignore)]
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    },
    Local {
        id: u64,
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
                    .get_token(config.client_timeout_secs, config.token_expire_secs)
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
                    client: NetClientDispatch::Netcode(client),
                    disconnect_reason: None,
                }
            }
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam {
                steamworks_client,
                config,
                conditioner,
            } => {
                let client = super::steam::client::Client::new(
                    steamworks_client.unwrap_or_else(|| {
                        Arc::new(RwLock::new(SteamworksClient::new(config.app_id)))
                    }),
                    config,
                    conditioner,
                );
                ClientConnection {
                    client: NetClientDispatch::Steam(client),
                    disconnect_reason: None,
                }
            }
            NetConfig::Local { id } => {
                let client = super::local::client::Client::new(id);
                ClientConnection {
                    client: NetClientDispatch::Local(client),
                    disconnect_reason: None,
                }
            }
        }
    }
}

impl NetClient for ClientConnection {
    fn connect(&mut self) -> Result<(), ConnectionError> {
        self.client.connect()
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.client.disconnect()
    }

    fn state(&self) -> ConnectionState {
        self.client.state()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<(), ConnectionError> {
        self.client.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<RecvPayload> {
        self.client.recv()
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), ConnectionError> {
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
/// Struct used to authenticate with the server when using the Netcode connection.
///
/// Netcode is a standard to establish secure connections between clients and game servers on top of
/// an unreliable unordered transport such as UDP.
/// You can read more about it here: `<https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md>`
///
/// The client sends a `ConnectToken` to the game server to start the connection process.
///
/// There are several ways to obtain a `ConnectToken`:
/// - the client can request a `ConnectToken` via a secure (e.g. HTTPS) connection from a backend server.
///   The server must use the same `protocol_id` and `private_key` as the game servers.
///   The backend server could be a dedicated webserver; or the game server itself, if it has a way to
///   establish secure connection.
/// - when testing, it can be convenient for the client to create its own `ConnectToken` manually.
///   You can use `Authentication::Manual` for those cases.
pub enum Authentication {
    /// Use a `ConnectToken` to authenticate with the game server.
    ///
    /// The client must have already received the `ConnectToken` from the backend.
    /// (The backend will generate a new `client_id` for the user, and use that to generate the
    /// `ConnectToken`)
    Token(ConnectToken),
    /// The client can build a `ConnectToken` manually.
    ///
    /// This is only useful for testing purposes. In production, the client should not have access
    /// to the `private_key`.
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
    #[default]
    /// The client has no `ConnectToken`, so it cannot connect to the game server yet.
    ///
    /// This is provided so that you can still build a [`ClientConnection`] `Resource` while waiting
    /// to receive a `ConnectToken` from the backend.
    None,
}

impl Authentication {
    /// Returns true if the Authentication contains a [`ConnectToken`] that can be used to
    /// connect to the game server
    pub fn has_token(&self) -> bool {
        !matches!(self, Authentication::None)
    }

    pub fn get_token(
        self,
        client_timeout_secs: i32,
        token_expire_secs: i32,
    ) -> Option<ConnectToken> {
        match self {
            Authentication::Token(token) => Some(token),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .timeout_seconds(client_timeout_secs)
                .expire_seconds(token_expire_secs)
                .generate()
                .ok(),
            Authentication::None => {
                // create a fake connect token so that we can build a NetcodeClient
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

impl std::fmt::Debug for Authentication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Authentication::Token(_) => write!(f, "Token(<connect_token>)"),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => f
                .debug_struct("Manual")
                .field("server_addr", server_addr)
                .field("client_id", client_id)
                .field("private_key", private_key)
                .field("protocol_id", protocol_id)
                .finish(),
            Authentication::None => write!(f, "None"),
        }
    }
}

/// Errors related to the client connection
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    #[error("io is not initialized")]
    IoNotInitialized,
    #[error("connection not found")]
    NotFound,
    #[error("client is not connected")]
    NotConnected,
    #[error(transparent)]
    Transport(#[from] crate::transport::error::Error),
    #[error("netcode error: {0}")]
    Netcode(#[from] super::netcode::error::Error),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamInvalidHandle(#[from] steamworks::networking_sockets::InvalidHandle),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamInvalidState(#[from] steamworks::networking_types::InvalidConnectionState),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamError(#[from] steamworks::SteamError),
}
