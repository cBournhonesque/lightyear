use anyhow::Result;
use bevy::prelude::Resource;
use bevy::utils::HashMap;

use crate::connection::id::ClientId;
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::connection::steam::server::SteamConfig;
use crate::packet::packet::Packet;
use crate::prelude::{Io, IoConfig, LinkConditionerConfig};
use crate::server::config::NetcodeConfig;

pub trait NetServer: Send + Sync {
    /// Start the server
    /// (i.e. start listening for client connections)
    fn start(&mut self) -> Result<()>;

    /// Stop the server
    /// (i.e. stop listening for client connections and stop all networking)
    fn stop(&mut self) -> Result<()>;

    // TODO: should we also have an API for accepting a client? i.e. we receive a connection request
    //  and we decide whether to accept it or not
    /// Disconnect a specific client
    fn disconnect(&mut self, client_id: ClientId) -> Result<()>;

    /// Return the list of connected clients
    fn connected_client_ids(&self) -> Vec<ClientId>;

    /// Update the connection states + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<()>;

    /// Receive a packet from one of the connected clients
    fn recv(&mut self) -> Option<(Packet, ClientId)>;

    /// Send a packet to one of the connected clients
    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()>;

    fn new_connections(&self) -> Vec<ClientId>;

    fn new_disconnections(&self) -> Vec<ClientId>;

    fn io(&self) -> &Io;
}

/// A wrapper around a `Box<dyn NetServer>`
#[derive(Resource)]
pub struct ServerConnection {
    server: Box<dyn NetServer>,
}

/// Configuration for the server connection
#[derive(Clone, Debug)]
pub enum NetConfig {
    Netcode {
        config: NetcodeConfig,
        io: IoConfig,
    },
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam {
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    },
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
                let io = io.build();
                let server = super::netcode::Server::new(config, io);
                ServerConnection {
                    server: Box::new(server),
                }
            }
            // TODO: might want to distinguish between steam with direct ip connections
            //  vs steam with p2p connections
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam {
                config,
                conditioner,
            } => {
                // TODO: handle errors
                let server = super::steam::server::Server::new(config, conditioner)
                    .expect("could not create steam server");
                ServerConnection {
                    server: Box::new(server),
                }
            }
        }
    }
}

impl NetServer for ServerConnection {
    fn start(&mut self) -> Result<()> {
        self.server.start()
    }

    fn stop(&mut self) -> Result<()> {
        self.server.stop()
    }

    fn disconnect(&mut self, client_id: ClientId) -> Result<()> {
        self.server.disconnect(client_id)
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.server.connected_client_ids()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        self.server.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<(Packet, ClientId)> {
        self.server.recv()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()> {
        self.server.send(buf, client_id)
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.server.new_connections()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.server.new_disconnections()
    }

    fn io(&self) -> &Io {
        self.server.io()
    }
}

type ServerConnectionIdx = usize;

// TODO: add a way to get the server of a given type?
/// On the server we allow the use of multiple types of ServerConnection at the same time
/// This resource holds the list of all the [`ServerConnection`]s, and maps client ids to the index of the server connection in the list
#[derive(Resource)]
pub struct ServerConnections {
    /// list of the various `ServerConnection`s available. Will be static after first insertion.
    servers: Vec<ServerConnection>,
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

    pub fn start(&mut self) -> Result<()> {
        for server in &mut self.servers {
            server.start()?;
        }
        self.is_listening = true;
        Ok(())
    }

    pub(crate) fn is_listening(&self) -> bool {
        self.is_listening
    }
}
