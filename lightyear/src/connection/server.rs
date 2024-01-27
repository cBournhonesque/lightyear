use crate::_reexport::ReadWordBuffer;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::netcode::ClientId;
use crate::prelude::Io;
use crate::server::config::NetcodeConfig;
use anyhow::Result;
use bevy::prelude::Resource;

pub trait NetServer: Send + Sync {
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

#[derive(Default, Clone)]
pub enum NetConfig {
    Netcode { config: NetcodeConfig, io: Io },
    // TODO: add steam-specific config
    Steam,
    Rivet { config: NetcodeConfig, io: Io },
}

impl NetConfig {
    pub fn get_server(self) -> ServerConnection {
        match self {
            NetConfig::Netcode { config, io } => {
                let server = super::netcode::Server::new(config, io);
                ServerConnection {
                    server: Box::new(server),
                }
            }
            NetConfig::Steam => {
                unimplemented!()
            }
            NetConfig::Rivet { config, io } => {
                let server = super::rivet::server::RivetServer {
                    netcode_server: super::netcode::Server::new(config, io),
                };
                ServerConnection {
                    server: Box::new(server),
                }
            }
        }
    }
}

impl NetServer for ServerConnection {
    fn connected_client_ids(&self) -> Vec<ClientId> {
        todo!()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        todo!()
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        todo!()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()> {
        todo!()
    }

    fn new_connections(&self) -> Vec<ClientId> {
        todo!()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        todo!()
    }
}
