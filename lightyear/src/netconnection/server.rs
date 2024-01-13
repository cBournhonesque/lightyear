use crate::_reexport::ReadWordBuffer;
use crate::netconnection::ClientId;
use crate::prelude::Io;
use steamworks::SteamError;

use anyhow::Result;

/// Event emitted on each server update, to notify a new client connections or disconnections.
pub struct ConnectionEvents {
    pub connected: Vec<ClientId>,
    pub disconnected: Vec<ClientId>,
}

pub trait NetServer {
    /// Return the list of connected clients
    fn connected_client_ids(&self) -> Vec<ClientId>;

    /// Update the connection states + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64, io: &mut Io) -> Result<ConnectionEvents>;

    /// Receive a packet from one of the connected clients
    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)>;

    /// Send a packet to one of the connected clients
    fn send(&mut self, buf: &[u8], client_id: ClientId, io: &mut Io) -> Result<(), Self::Error>;
}
