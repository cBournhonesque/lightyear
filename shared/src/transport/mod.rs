//! Interface for the transport layer
mod conditioner;

use anyhow::Result;


pub trait Transport: PacketReceiver + PacketSender {}
pub trait PacketSender {
    /// Send data on the socket to the remote address to which it is connected
    /// Fails if the socket is not connected
    fn send(&self, payload: &[u8]) -> Result<()>;
}
pub trait PacketReceiver {
    /// Receive a packet from the socket from the remote address to which it is connected
    ///
    /// Fails if the socket is not connected
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<&[u8]>>;
}
