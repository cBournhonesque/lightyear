//! Client connection abstraction with Rivet
//!
//! The client should:
//! 1. call the Rivet matchmaker to get a player token, and the server and backend's address
//! 2. call the backend to get a connect token
//! 3. connect to the dedicated server using netcode with the connect token

use std::net::SocketAddr;
use std::str::FromStr;

use anyhow::Context;
use tracing::info;

use crate::_reexport::ReadWordBuffer;
use crate::client::config::NetcodeConfig;
use crate::connection::client::NetClient;
use crate::connection::netcode::{Client, ClientId, NetcodeClient};
use crate::connection::rivet::matchmaker;
use crate::prelude::Io;

/// Wrapper around the netcode client that uses Rivet to get the server address
/// and calls a http backend server to get a `ConnectToken`
pub struct RivetClient {
    pub(crate) netcode_config: NetcodeConfig,
    pub(crate) io: Option<Io>,
    pub(crate) netcode_client: Option<Client<()>>,
}

impl NetClient for RivetClient {
    fn connect(&mut self) -> anyhow::Result<()> {
        // 1. call the Rivet matchmaker to get a player token, and the server and backend's address
        let rivet_server_data = matchmaker::find_lobby()?;
        info!("rivet_server_data: {:?}", rivet_server_data);

        let backend_host = rivet_server_data["backend"]["host"]
            .as_str()
            .context("could not parse the backend host")?;
        let backend_port = rivet_server_data["backend"]["port"]
            .as_u64()
            .context("could not parse the backend port")? as u16;
        let backend_addr = SocketAddr::from_str(&*format!("{}:{}", backend_host, backend_port))?;

        // 2. call the backend to get a connect token
        let client = reqwest::blocking::Client::new();
        let token_bytes = client
            .post(format!("{}/connect", backend_addr.to_string()))
            .json(&rivet_server_data)
            .send()?
            .error_for_status()?
            .bytes()?;

        // 3. create a Netcode client from the connect token
        let netcode_client =
            NetcodeClient::with_config(token_bytes.as_ref(), self.netcode_config.build())?;
        let io = std::mem::take(&mut self.io).unwrap();
        let mut netcode_client = Client {
            client: netcode_client,
            io,
        };

        // 4. connect to the dedicated server
        netcode_client.connect()?;
        self.netcode_client = Some(netcode_client);

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.netcode_client.as_ref().unwrap().is_connected()
    }

    fn try_update(&mut self, delta_ms: f64) -> anyhow::Result<()> {
        self.netcode_client.as_mut().unwrap().try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<ReadWordBuffer> {
        self.netcode_client.as_mut().unwrap().recv()
    }

    fn send(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.netcode_client.as_mut().unwrap().send(buf)
    }

    fn id(&self) -> ClientId {
        self.netcode_client.as_ref().unwrap().id()
    }

    fn local_addr(&self) -> SocketAddr {
        self.io.as_ref().unwrap().local_addr()
    }

    fn io(&self) -> &Io {
        self.io.as_ref().unwrap()
    }

    fn io_mut(&mut self) -> &mut Io {
        self.io.as_mut().unwrap()
    }
}
