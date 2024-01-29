use crate::_reexport::ReadWordBuffer;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::netcode::ClientId;
use crate::connection::rivet::backend::RivetBackend;
use crate::prelude::Io;
use crate::server::config::NetcodeConfig;
use anyhow::Result;
use bevy::prelude::Resource;

pub trait NetServer: Send + Sync {
    /// Start the server
    fn start(&mut self);

    /// Return the list of connected clients
    fn connected_client_ids(&self) -> Vec<ClientId>;

    /// Update the connection states + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<()>;

    /// Receive a packet from one of the connected clients
    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)>;

    /// Send a packet to one of the connected clients
    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()>;

    fn new_connections(&self) -> Vec<ClientId>;

    fn new_disconnections(&self) -> Vec<ClientId>;
}

#[derive(Resource)]
pub struct ServerConnection {
    server: Box<dyn NetServer>,
}

#[derive(Clone)]
pub enum NetConfig {
    Netcode {
        config: NetcodeConfig,
    },
    // TODO: add steam-specific config
    Steam,
    #[cfg(feature = "rivet")]
    Rivet {
        config: NetcodeConfig,
    },
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig::Netcode {
            config: NetcodeConfig::default(),
        }
    }
}

impl NetConfig {
    pub fn get_server(self, io: Io) -> ServerConnection {
        match self {
            NetConfig::Netcode { config } => {
                let server = super::netcode::Server::new(config, io);
                ServerConnection {
                    server: Box::new(server),
                }
            }
            NetConfig::Steam => {
                unimplemented!()
            }
            #[cfg(feature = "rivet")]
            NetConfig::Rivet { config } => {
                let server = super::rivet::server::RivetServer {
                    netcode_server: super::netcode::Server::new(config, io),
                    backend: Some(RivetBackend),
                };
                ServerConnection {
                    server: Box::new(server),
                }
            }
        }
    }
}

impl NetServer for ServerConnection {
    fn start(&mut self) {
        self.server.start()
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.server.connected_client_ids()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        self.server.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
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
}
