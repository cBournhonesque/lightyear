//! The transport layer is responsible for sending and receiving raw byte arrays packets through the network.

/// io is a wrapper around the underlying transport layer
pub mod io;

/// The transport is a local channel
pub(crate) mod local;

/// The transport is a UDP socket
#[cfg_attr(docsrs, doc(cfg(not(target_family = "wasm"))))]
#[cfg(not(target_family = "wasm"))]
pub(crate) mod udp;

#[cfg(target_family = "wasm")]
mod certificate;

/// The transport is a map of channels (used for server, during testing)
pub(crate) mod channels;

/// The transport is using WebTransport
#[cfg_attr(docsrs, doc(cfg(feature = "webtransport")))]
#[cfg(feature = "webtransport")]
pub(crate) mod webtransport;

pub(crate) mod dummy;
pub(crate) mod error;
#[cfg_attr(docsrs, doc(cfg(feature = "websocket")))]
#[cfg(feature = "websocket")]
pub(crate) mod websocket;
pub(crate) mod wrapper;

use error::Result;
use std::net::SocketAddr;

pub const LOCAL_SOCKET: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
    0,
);
// Maximum transmission units; maximum size in bytes of a UDP packet
// See: https://gafferongames.com/post/packet_fragmentation_and_reassembly/
pub(crate) const MTU: usize = 1472;

/// Transport combines a PacketSender and a PacketReceiver
///
/// This trait is used to abstract the raw transport layer that sends and receives packets.
/// There are multiple implementations of this trait, such as UdpSocket, WebSocket, WebTransport, etc.
pub trait Transport: Send + Sync {
    // type Sender: PacketSender;
    // type Receiver: PacketReceiver;
    /// Return the local socket address for this transport
    fn local_addr(&self) -> SocketAddr;

    // TODO: should this function be async?
    /// Connect to the remote address
    fn connect(&mut self) -> Result<()>;

    /// Return the [`PacketSender`] and [`PacketReceiver`] for this transport
    /// (this is useful to have a mutable reference to the sender and receiver at the same time)
    fn split(&mut self) -> (&mut (dyn PacketSender + '_), &mut (dyn PacketReceiver + '_));

    // fn split(&mut self) -> (&mut impl PacketSender, &mut impl PacketReceiver);

    // TODO maybe add a `async fn ready() -> bool` function?
}

/// Send data to a remote address
pub trait PacketSender: Send + Sync {
    /// Send data on the socket to the remote address
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()>;
}

/// Receive data from a remote address
pub trait PacketReceiver: Send + Sync {
    /// Receive a packet from the socket. Returns the data read and the origin.
    ///
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>>;
}

impl PacketSender for Box<dyn PacketSender> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }
}

// impl<T: PacketSender> PacketSender for &mut T {
//     fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
//         (*self).send(payload, address)
//     }
// }

impl PacketReceiver for Box<dyn PacketReceiver> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (**self).recv()
    }
}

// impl<T: PacketReceiver> PacketReceiver for &mut T {
//     fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
//         (*self).recv()
//     }
// }

impl PacketSender for &mut dyn PacketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (*self).send(payload, address)
    }
}

impl PacketSender for Box<&mut dyn PacketSender> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }
}

impl PacketReceiver for &mut dyn PacketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (*self).recv()
    }
}

impl PacketReceiver for Box<&mut dyn PacketReceiver> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (**self).recv()
    }
}
