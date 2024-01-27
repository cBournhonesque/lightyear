use crate::_reexport::ReadWordBuffer;
use crate::prelude::{Io, IoConfig, TransportConfig};

use crate::client::config::NetcodeConfig;
use crate::connection::netcode::ClientId;
use crate::prelude::client::Authentication;
use anyhow::Result;
use bevy::prelude::Resource;

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
    fn recv(&mut self) -> Option<ReadWordBuffer>;

    /// Send a packet to the server
    fn send(&mut self, buf: &[u8]) -> Result<()>;

    /// Get the id of the client
    fn id(&self) -> ClientId;
}

#[derive(Resource)]
pub struct ClientConnection {
    pub(crate) client: Box<dyn NetClient>,
}

pub enum NetConfig {
    Netcode {
        auth: Authentication,
        config: NetcodeConfig,
        io: Io,
    },
    // TODO: add steam-specific config
    Steam,
    Rivet {
        config: NetcodeConfig,
        io: Io,
    },
}

impl NetConfig {
    pub fn get_client(self) -> ClientConnection {
        match self {
            NetConfig::Netcode { auth, config, io } => {
                let config_clone = config.clone();
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
                    io,
                };
                ClientConnection {
                    client: Box::new(client),
                }
            }
            NetConfig::Steam => {
                unimplemented!()
                // // TODO: handle errors
                // let (steam_client, _) = steamworks::Client::init().unwrap();
                // Box::new(super::steam::Client::new(steam_client))
            }
            NetConfig::Rivet { config, io } => {
                let netcode = super::rivet::client::RivetClient {
                    netcode_config: config,
                    io: Some(io),
                    netcode_client: None,
                };
                ClientConnection {
                    client: Box::new(netcode),
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
        todo!()
    }

    fn recv(&mut self) -> Option<ReadWordBuffer> {
        todo!()
    }

    fn send(&mut self, buf: &[u8]) -> Result<()> {
        todo!()
    }

    fn id(&self) -> ClientId {
        todo!()
    }
}
