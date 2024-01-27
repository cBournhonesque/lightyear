//! Client connection abstraction with Rivet
//!
//! The client should:
//! 1. call the Rivet matchmaker to get a player token, and the server and backend's address
//! 2. call the backend to get a connect token
//! 3. connect to the dedicated server using netcode with the connect token

use crate::_reexport::ReadWordBuffer;
use crate::client::config::{ClientConfig, NetcodeConfig};
use crate::connection::client::NetClient;
use crate::connection::netcode::{Client, ClientId, ConnectToken, NetcodeClient};
use crate::connection::rivet::matchmaker;
use crate::prelude::{Io, IoConfig};
use std::net::SocketAddr;

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
        println!("rivet_server_data: {:?}", rivet_server_data);

        let backend_host = rivet_server_data["backend"]["host"].as_str()?;
        let backend_port = rivet_server_data["backend"]["port"].as_u64()? as u16;
        let backend_addr = SocketAddr::new(backend_host.into(), backend_port);

        // 2. call the backend to get a connect token
        let client = reqwest::blocking::Client::new();
        let token_bytes = client
            .post(format!(
                "{}/matchmaker/lobbies/ready",
                backend_addr.to_string()
            ))
            .json(&rivet_server_data)
            .send()?
            .error_for_status()?
            .bytes()?;

        // 3. create a Netcode client from the connect token
        let netcode_client =
            NetcodeClient::with_config(token_bytes.as_ref(), self.netcode_config.build())?;
        let io = std::mem::take(&mut self.io).unwrap();
        self.netcode_client = Some(Client {
            client: netcode_client,
            io,
        });

        // 4. connect to the dedicated server
        self.netcode_client.unwrap().connect()
    }

    fn is_connected(&self) -> bool {
        self.netcode_client.unwrap().is_connected()
    }

    fn try_update(&mut self, delta_ms: f64) -> anyhow::Result<()> {
        self.netcode_client.unwrap().try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<ReadWordBuffer> {
        self.netcode_client.unwrap().recv()
    }

    fn send(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        self.netcode_client.unwrap().send(buf)
    }

    fn id(&self) -> ClientId {
        self.netcode_client.unwrap().id()
    }

    fn local_addr(&self) -> SocketAddr {
        self.io.unwrap().local_addr()
    }

    fn io(&self) -> &Io {
        &self.io.unwrap()
    }

    fn io_mut(&mut self) -> &mut Io {
        &mut self.io.unwrap()
    }
}
