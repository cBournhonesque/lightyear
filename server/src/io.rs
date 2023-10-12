use std::io::Result;
use std::net::SocketAddr;

use crossbeam_channel::Sender;

use lightyear_shared::netcode::ClientIndex;
use lightyear_shared::transport::PacketSender;
use lightyear_shared::Io;

/// Wrapper around using the netcode.io protocol with a given transport
pub struct ServerIO<'i, 'n> {
    pub(crate) io: &'i mut Io,
    pub(crate) netcode: &'n mut lightyear_shared::netcode::Server<NetcodeServerContext>,
}

impl PacketSender for ServerIO<'_, '_> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        let client_index = self
            .netcode
            .client_index(address)
            .ok_or_else(|| std::io::Error::other("client not found"))?;
        self.netcode
            .send(payload, client_index, &mut self.io)
            .map_err(|e| std::io::Error::other(e))
    }
}

pub struct NetcodeServerContext {
    pub connections: Sender<ClientIndex>,
    pub disconnections: Sender<ClientIndex>,
}
