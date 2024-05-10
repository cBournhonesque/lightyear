//! The transport layer is responsible for sending and receiving raw byte arrays packets through the network.

use std::net::SocketAddr;

use enum_dispatch::enum_dispatch;

use error::Result;

use crate::transport::channels::Channels;
use crate::transport::dummy::DummyIo;
use crate::transport::io::IoState;
use crate::transport::local::{LocalChannel, LocalChannelBuilder};
#[cfg(not(target_family = "wasm"))]
use crate::transport::udp::{UdpSocket, UdpSocketBuilder};
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
#[cfg_attr(docsrs, doc(cfg(not(target_family = "wasm"))))]
#[cfg(not(target_family = "wasm"))]
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

pub(crate) type BoxedSender = Box<dyn PacketSender + Send + Sync>;
pub(crate) type BoxedReceiver = Box<dyn PacketReceiver + Send + Sync>;
// pub(crate) trait CloseFn: Send + Sync {}
// impl<T: Fn() -> Result<()> + Send + Sync> CloseFn for T {}
// pub(crate) type BoxedCloseFn = Box<dyn CloseFn>;
pub(crate) type BoxedCloseFn = Box<dyn (Fn() -> Result<()>) + Send + Sync>;

#[enum_dispatch]
pub(crate) trait Transport {
    /// Return the local socket address for this transport
    fn local_addr(&self) -> SocketAddr;

    /// Split the transport into a sender, receiver.
    ///
    /// This is useful to have parallel mutable access to the sender and the retriever
    fn split(self) -> (BoxedSender, BoxedReceiver);
}

pub(crate) mod client {
    use super::*;
    use crate::transport::io::{ClientIoEventReceiver, ClientNetworkEventSender};

    /// Transport combines a PacketSender and a PacketReceiver
    ///
    /// This trait is used to abstract the raw transport layer that sends and receives packets.
    /// There are multiple implementations of this trait, such as UdpSocket, WebSocket, WebTransport, etc.
    #[enum_dispatch]
    pub(crate) trait ClientTransportBuilder: Send + Sync {
        /// Attempt to connect to the remote
        fn connect(
            self,
        ) -> Result<(
            ClientTransportEnum,
            IoState,
            Option<ClientIoEventReceiver>,
            Option<ClientNetworkEventSender>,
        )>;
    }

    #[enum_dispatch(ClientTransportBuilder)]
    pub(crate) enum ClientTransportBuilderEnum {
        #[cfg(not(target_family = "wasm"))]
        UdpSocket(UdpSocketBuilder),
        #[cfg(feature = "webtransport")]
        WebTransportClient(WebTransportClientSocketBuilder),
        #[cfg(feature = "websocket")]
        WebSocketClient(WebSocketClientSocketBuilder),
        LocalChannel(LocalChannelBuilder),
        Dummy(DummyIo),
    }

    #[allow(clippy::large_enum_variant)]
    #[enum_dispatch(Transport)]
    pub(crate) enum ClientTransportEnum {
        #[cfg(not(target_family = "wasm"))]
        UdpSocket(UdpSocket),
        #[cfg(feature = "webtransport")]
        WebTransportClient(WebTransportClientSocket),
        #[cfg(feature = "websocket")]
        WebSocketClient(WebSocketClientSocket),
        LocalChannel(LocalChannel),
        Dummy(DummyIo),
    }
}

pub(crate) mod server {
    use super::*;
    use crate::transport::io::{ServerIoEventReceiver, ServerNetworkEventSender};

    #[enum_dispatch]
    pub(crate) trait ServerTransportBuilder: Send + Sync {
        /// Attempt to listen for incoming connections
        fn start(
            self,
        ) -> Result<(
            ServerTransportEnum,
            IoState,
            Option<ServerIoEventReceiver>,
            Option<ServerNetworkEventSender>,
        )>;
    }

    #[enum_dispatch(ServerTransportBuilder)]
    pub(crate) enum ServerTransportBuilderEnum {
        #[cfg(not(target_family = "wasm"))]
        UdpSocket(UdpSocketBuilder),
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        WebTransportServer(WebTransportServerSocketBuilder),
        #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
        WebSocketServer(WebSocketServerSocketBuilder),
        Channels(Channels),
        Dummy(DummyIo),
    }

    #[allow(clippy::large_enum_variant)]
    #[enum_dispatch(Transport)]
    pub(crate) enum ServerTransportEnum {
        #[cfg(not(target_family = "wasm"))]
        UdpSocket(UdpSocket),
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        WebTransportServer(WebTransportServerSocket),
        #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
        WebSocketServer(WebSocketServerSocket),
        Channels(Channels),
        Dummy(DummyIo),
    }
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
