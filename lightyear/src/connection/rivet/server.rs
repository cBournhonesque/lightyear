use crate::_reexport::ReadWordBuffer;
use crate::connection::server::NetServer;
use crate::prelude::ClientId;

pub struct RivetServer {
    pub(crate) netcode_server: crate::connection::netcode::Server,
}

impl NetServer for RivetServer {
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
