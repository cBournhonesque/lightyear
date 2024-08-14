//! The transport layer is responsible for sending and receiving raw byte arrays packets through the network.
#![allow(unused_imports)]

use std::net::SocketAddr;

use enum_dispatch::enum_dispatch;

use error::Result;

// required import for enum dispatch to work
use crate::client::io::transport::ClientTransportEnum;
use crate::server::io::transport::ServerTransportEnum;
use crate::transport::channels::Channels;
use crate::transport::dummy::DummyIo;
use crate::transport::local::LocalChannel;
use crate::transport::udp::UdpSocket;
#[cfg(feature = "websocket")]
use crate::transport::websocket::client::{WebSocketClientSocket, WebSocketClientSocketBuilder};
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::{WebSocketServerSocket, WebSocketServerSocketBuilder};
#[cfg(feature = "webtransport")]
use crate::transport::webtransport::client::{
    WebTransportClientSocket, WebTransportClientSocketBuilder,
};
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use crate::transport::webtransport::server::{
    WebTransportServerSocket, WebTransportServerSocketBuilder,
};

/// io is a wrapper around the underlying transport layer
pub mod io;

/// The transport is a local channel
pub(crate) mod local;

/// The transport is a UDP socket
pub(crate) mod udp;

/// The transport is a map of channels (used for server, during testing)
pub(crate) mod channels;

/// The transport is using WebTransport
#[cfg_attr(docsrs, doc(cfg(feature = "webtransport")))]
#[cfg(feature = "webtransport")]
pub(crate) mod webtransport;

pub(crate) mod middleware;

pub mod config;
pub(crate) mod dummy;
pub(crate) mod error;
#[cfg_attr(docsrs, doc(cfg(feature = "websocket")))]
#[cfg(feature = "websocket")]
pub(crate) mod websocket;

pub const LOCAL_SOCKET: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
    0,
);
/// Maximum transmission units; maximum size in bytes of a UDP packet
/// See: <https://gafferongames.com/post/packet_fragmentation_and_reassembly/>
pub(crate) const MTU: usize = 1472;

/// Minimum MTU used by QUIC. Any packets bigger than this will error with TooLarge
/// There is MTU Discovery to potentially allow bigger MTUs, and this is the minimum
/// the discovery will start from.
pub(crate) const MIN_MTU: usize = 1300;

pub(crate) type BoxedSender = Box<dyn PacketSender + Send + Sync>;
pub(crate) type BoxedReceiver = Box<dyn PacketReceiver + Send + Sync>;

#[enum_dispatch]
pub(crate) trait Transport {
    /// Return the local socket address for this transport
    fn local_addr(&self) -> SocketAddr;

    /// Split the transport into a sender, receiver.
    ///
    /// This is useful to have parallel mutable access to the sender and the retriever
    fn split(self) -> (BoxedSender, BoxedReceiver);
}

/// Send data to a remote address
pub trait PacketSender: Send + Sync {
    /// Send data on the socket to the remote address
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()>;
}

impl PacketSender for BoxedSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }
}

/// Receive data from a remote address
pub trait PacketReceiver: Send + Sync {
    /// Receive a packet from the socket. Returns the data read and the origin.
    ///
    /// Returns Ok(None) if no data is available
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>>;
}

impl PacketReceiver for BoxedReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (**self).recv()
    }
}
