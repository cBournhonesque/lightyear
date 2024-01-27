use crate::_reexport::ReadWordBuffer;
use crate::prelude::Io;

use crate::client::config::NetcodeConfig;
use crate::prelude::client::Authentication;
use anyhow::Result;

pub trait NetClient {
    // type Error;

    /// Connect to server
    fn connect(&mut self);
    fn is_connected(&self) -> bool;

    /// Update the connection state + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64, io: &mut Io) -> Result<()>;

    /// Receive a packet from the server
    fn recv(&mut self) -> Option<ReadWordBuffer>;

    /// Send a packet to the server
    fn send(&mut self, buf: &[u8], io: &mut Io) -> Result<()>;
}

pub enum NetConfig {
    Netcode {
        auth: Authentication,
        config: NetcodeConfig,
    },
    // TODO: add steam-specific config
    Steam,
}

impl NetConfig {
    pub fn get_client(self) -> Box<dyn NetClient> {
        match self {
            NetConfig::Netcode { auth, config } => {
                let config_clone = config.clone();
                let token = auth
                    .clone()
                    .get_token(config.client_timeout_secs)
                    .expect("could not generate token");
                let token_bytes = token.try_into_bytes().unwrap();
                let netcode = super::netcode::Client::with_config(&token_bytes, config.build())
                    .expect("could not create netcode client");
                Box::new(netcode)
            }
            NetConfig::Steam => {
                // TODO: handle errors
                let (steam_client, _) = steamworks::Client::init().unwrap();
                Box::new(super::steam::Client::new(steam_client))
            }
        }
    }
}
