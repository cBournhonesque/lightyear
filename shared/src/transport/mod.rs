//! Interface for the transport layer
use std::io::Result;
use std::net::SocketAddr;

use crate::ReadBuffer;

pub(crate) mod conditioner;
pub(crate) mod io;
pub(crate) mod udp;

pub trait Transport: PacketReceiver + PacketSender {
    /// Return the local socket address for this transport
    fn local_addr(&self) -> SocketAddr;

    // fn split(&mut self) -> (Box<dyn PacketReceiver>, Box<dyn PacketSender>);
}

pub trait PacketSender {
    /// Send data on the socket to the remote address
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()>;
}

pub trait PacketReceiver {
    /// Receive a packet from the socket. Returns the data read and the origin.
    ///
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>>;
}
