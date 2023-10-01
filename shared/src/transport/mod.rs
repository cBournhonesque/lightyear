//! Interface for the transport layer
use std::net::SocketAddr;

use anyhow::Result;

mod conditioner;
pub(crate) mod udp;

pub trait Transport: PacketReceiver + PacketSender {
    /// Return the local socket address for this transport
    fn local_addr(&self) -> Result<SocketAddr>;
}
pub trait PacketSender {
    /// Send data on the socket to the remote address
    fn send(&self, payload: &[u8], address: &SocketAddr) -> Result<()>;
}
pub trait PacketReceiver {
    /// Receive a packet from the socket. Returns the data read and the origin.
    ///
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<(&[u8], SocketAddr)>>;
}
