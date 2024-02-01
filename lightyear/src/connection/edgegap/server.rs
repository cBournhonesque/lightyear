use tracing::error;

use crate::_reexport::ReadWordBuffer;
use crate::connection::server::NetServer;
use crate::prelude::ClientId;

pub struct RivetServer {
    pub(crate) netcode_server: crate::connection::netcode::Server,
    pub(crate) backend: Option<super::backend::RivetBackend>,
}

impl NetServer for RivetServer {
    fn start(&mut self) {
        let backend = std::mem::take(&mut self.backend).unwrap();
        // spawn the backend http service
        tokio::spawn(async move {
            backend.serve().await;
        });
        // notify the rivet matchmaker that the server is ready
        // TODO: should this be done somewhere else?
        tokio::spawn(async move {
            super::matchmaker::lobby_ready().await.map_err(|e| {
                error!("could not set the lobby as ready: {:?}", e);
            })
        });
    }
    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.netcode_server.connected_client_ids()
    }

    fn try_update(&mut self, delta_ms: f64) -> anyhow::Result<()> {
        self.netcode_server.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        self.netcode_server.recv()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> anyhow::Result<()> {
        self.netcode_server.send(buf, client_id)
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.netcode_server.new_connections()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.netcode_server.new_disconnections()
    }
}
