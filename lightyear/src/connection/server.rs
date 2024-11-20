use bevy::prelude::Resource;
use bevy::utils::HashMap;
use enum_dispatch::enum_dispatch;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;

use crate::connection::id::ClientId;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::connection::steam::{server::SteamConfig, steamworks_client::SteamworksClient};
use crate::packet::packet_builder::RecvPayload;
use crate::prelude::server::ServerTransport;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::prelude::LinkConditionerConfig;
use crate::server::config::NetcodeConfig;
use crate::server::io::Io;
use crate::transport::config::SharedIoConfig;

/// Reasons for denying a connection request
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DeniedReason {
    ServerFull,
    Banned,
    InternalError,
    AlreadyConnected,
    TokenAlreadyUsed,
    InvalidToken,
    Custom(String),
}

/// Trait for handling connection requests from clients.
pub trait ConnectionRequestHandler: Debug + Send + Sync {
    /// Handle a connection request from a client.
    /// Returns None if the connection is accepted,
    /// Returns Some(reason) if the connection is denied.
    fn handle_request(&self, client_id: ClientId) -> Option<DeniedReason>;
}

/// By default, all connection requests are accepted by the server.
#[derive(Debug, Clone)]
pub struct DefaultConnectionRequestHandler;

impl ConnectionRequestHandler for DefaultConnectionRequestHandler {
    fn handle_request(&self, client_id: ClientId) -> Option<DeniedReason> {
        None
    }
}

#[enum_dispatch]
pub trait NetServer: Send + Sync {
    /// Start the server
    /// (i.e. start listening for client connections)
    fn start(&mut self) -> Result<(), ConnectionError>;

    /// Stop the server
    /// (i.e. stop listening for client connections and stop all networking)
    fn stop(&mut self) -> Result<(), ConnectionError>;

    // TODO: should we also have an API for accepting a client? i.e. we receive a connection request
    //  and we decide whether to accept it or not
    /// Disconnect a specific client
    /// Is also responsible for adding the client to the list of new disconnections.
    fn disconnect(&mut self, client_id: ClientId) -> Result<(), ConnectionError>;

    /// Return the list of connected clients
    fn connected_client_ids(&self) -> Vec<ClientId>;

    /// Update the connection states + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<(), ConnectionError>;

    /// Receive a packet from one of the connected clients
    fn recv(&mut self) -> Option<(RecvPayload, ClientId)>;

    /// Send a packet to one of the connected clients
    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<(), ConnectionError>;

    fn new_connections(&self) -> Vec<ClientId>;

    fn new_disconnections(&self) -> Vec<ClientId>;

    fn io(&self) -> Option<&Io>;

    fn io_mut(&mut self) -> Option<&mut Io>;
}

#[enum_dispatch(NetServer)]
pub enum ServerConnection {
    Netcode(super::netcode::Server),
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam(super::steam::server::Server),
}

pub type IoConfig = SharedIoConfig<ServerTransport>;

/// Configuration for the server connection
#[derive(Clone, Debug)]
pub enum NetConfig {
    Netcode {
        config: NetcodeConfig,
        io: IoConfig,
    },
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam {
        steamworks_client: Option<Arc<RwLock<SteamworksClient>>>,
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    },
}

impl NetConfig {
    /// Update the `accept_connection_request_fn` field in the config
    pub fn set_connection_request_handler(
        &mut self,
        connection_request_handler: Arc<dyn ConnectionRequestHandler>,
    ) {
        match self {
            NetConfig::Netcode { config, .. } => {
                config.connection_request_handler = connection_request_handler;
            }
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam { config, .. } => {
                config.connection_request_handler = connection_request_handler;
            }
        }
    }
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig::Netcode {
            config: NetcodeConfig::default(),
            io: IoConfig::default(),
        }
    }
}

impl NetConfig {
    pub fn build_server(self) -> ServerConnection {
        match self {
            NetConfig::Netcode { config, io } => {
                let server = super::netcode::Server::new(config, io);
                ServerConnection::Netcode(server)
            }
            // TODO: might want to distinguish between steam with direct ip connections
            //  vs steam with p2p connections
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam {
                steamworks_client,
                config,
                conditioner,
            } => {
                // TODO: handle errors
                let server = super::steam::server::Server::new(
                    steamworks_client.unwrap_or_else(|| {
                        Arc::new(RwLock::new(SteamworksClient::new(config.app_id)))
                    }),
                    config,
                    conditioner,
                )
                .expect("could not create steam server");
                ServerConnection::Steam(server)
            }
        }
    }
}

type ServerConnectionIdx = usize;

// TODO: add a way to get the server of a given type?
/// On the server we allow the use of multiple types of ServerConnection at the same time
/// This resource holds the list of all the [`ServerConnection`]s, and maps client ids to the index of the server connection in the list
#[derive(Resource)]
pub struct ServerConnections {
    /// list of the various `ServerConnection`s available. Will be static after first insertion.
    pub servers: Vec<ServerConnection>,
    /// Mapping from the connection's [`ClientId`] into the index of the [`ServerConnection`] in the `servers` list
    pub(crate) client_server_map: HashMap<ClientId, ServerConnectionIdx>,
    /// Track whether the server is ready to listen to incoming connections
    is_listening: bool,
}

impl ServerConnections {
    pub fn new(config: Vec<NetConfig>) -> Self {
        let mut servers = vec![];
        for config in config {
            let server = config.build_server();
            servers.push(server);
        }
        ServerConnections {
            servers,
            client_server_map: HashMap::default(),
            is_listening: false,
        }
    }

    /// Start listening for client connections on all internal servers
    pub fn start(&mut self) -> Result<(), ConnectionError> {
        for server in &mut self.servers {
            server.start()?;
        }
        self.is_listening = true;
        Ok(())
    }

    /// Stop listening for client connections on all internal servers
    pub fn stop(&mut self) -> Result<(), ConnectionError> {
        for server in &mut self.servers {
            server.stop()?;
        }
        self.is_listening = false;
        Ok(())
    }

    /// Disconnect a specific client
    pub fn disconnect(&mut self, client_id: ClientId) -> Result<(), ConnectionError> {
        self.client_server_map.get(&client_id).map_or(
            Err(ConnectionError::ConnectionNotFound),
            |&server_idx| {
                self.servers[server_idx].disconnect(client_id)?;
                // we are not removing the client_id from the client_server_map here
                // because we still need it there to be able to send disconnect packets
                // the client_id gets removed in the server's receive_packets function
                Ok(())
            },
        )
    }

    /// Returns true if the server is currently listening for client packets
    pub(crate) fn is_listening(&self) -> bool {
        self.is_listening
    }
}

/// Errors related to the server connection
#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    #[error("io is not initialized")]
    IoNotInitialized,
    #[error("connection not found")]
    ConnectionNotFound,
    #[error("the connection type for this client is invalid")]
    InvalidConnectionType,
    #[error(transparent)]
    Transport(#[from] crate::transport::error::Error),
    #[error("netcode error: {0}")]
    Netcode(#[from] super::netcode::error::Error),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamInvalidHandle(#[from] steamworks::networking_sockets::InvalidHandle),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamInitError(#[from] steamworks::SteamAPIInitError),
    #[error(transparent)]
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    SteamError(#[from] steamworks::SteamError),
}

#[cfg(test)]
mod tests {
    use crate::connection::server::{NetServer, ServerConnections};
    use crate::prelude::ClientId;
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};

    // Check that the server can successfully disconnect a client
    // and that there aren't any excessive logs afterwards
    // Enable logging to see if the logspam is fixed!
    #[test]
    fn test_server_disconnect_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let mut stepper = BevyStepper::default();
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ServerConnections>()
            .disconnect(ClientId::Netcode(TEST_CLIENT_ID))
            .unwrap();
        // make sure the server disconnected the client
        for _ in 0..10 {
            stepper.frame_step();
        }
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .resource_mut::<ServerConnections>()
                .servers[0]
                .connected_client_ids(),
            vec![]
        );
    }
}
