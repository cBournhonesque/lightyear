use bevy::prelude::Reflect;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, SocketAddr};

use crossbeam_channel::{Receiver, Sender};

#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use {
    crate::transport::webtransport::server::WebTransportServerSocketBuilder,
    wtransport::tls::Certificate,
};

use crate::prelude::Io;
use crate::transport::channels::Channels;
use crate::transport::dummy::DummyIo;
use crate::transport::local::LocalChannelBuilder;
use crate::transport::middleware::conditioner::{LinkConditioner, LinkConditionerConfig};
#[cfg(not(target_family = "wasm"))]
use crate::transport::udp::UdpSocketBuilder;
#[cfg(feature = "websocket")]
use crate::transport::websocket::client::WebSocketClientSocketBuilder;
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::WebSocketServerSocketBuilder;
#[cfg(feature = "webtransport")]
use crate::transport::webtransport::client::WebTransportClientSocketBuilder;
use crate::transport::{Transport, TransportBuilderEnum};

/// Use this to configure the [`Transport`] that will be used to establish a connection with the
/// remote.
#[derive(Clone)]
pub enum TransportConfig {
    /// Use a [`UdpSocket`](std::net::UdpSocket)
    #[cfg(not(target_family = "wasm"))]
    UdpSocket(SocketAddr),
    /// Use [`WebTransport`](https://wicg.github.io/web-transport/) as a transport layer
    #[cfg(feature = "webtransport")]
    WebTransportClient {
        client_addr: SocketAddr,
        server_addr: SocketAddr,
        /// On wasm, we need to provide a hash of the certificate to the browser
        #[cfg(target_family = "wasm")]
        certificate_digest: String,
    },
    /// Use [`WebTransport`](https://wicg.github.io/web-transport/) as a transport layer
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    WebTransportServer {
        server_addr: SocketAddr,
        /// Certificate that will be used for authentication
        certificate: Certificate,
    },
    /// Use [`WebSocket`](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket) as a transport
    #[cfg(feature = "websocket")]
    WebSocketClient { server_addr: SocketAddr },
    /// Use [`WebSocket`](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket) as a transport
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    WebSocketServer { server_addr: SocketAddr },
    /// Use [`Channels`](crossbeam_channel::channel) as a transport. This is useful for testing.
    /// This is server-only: each tuple corresponds to a different client.
    Channels {
        channels: Vec<(SocketAddr, Receiver<Vec<u8>>, Sender<Vec<u8>>)>,
    },
    /// Use [`Channels`](crossbeam_channel::channel) as a transport. This is useful for testing.
    /// This is mostly for clients.
    LocalChannel {
        recv: Receiver<Vec<u8>>,
        send: Sender<Vec<u8>>,
    },
    /// Dummy transport if the connection handles its own io (for example steam sockets)
    Dummy,
}

impl TransportConfig {
    fn build(self) -> TransportBuilderEnum {
        match self {
            #[cfg(not(target_family = "wasm"))]
            TransportConfig::UdpSocket(addr) => {
                TransportBuilderEnum::UdpSocket(UdpSocketBuilder { local_addr: addr })
            }
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
            } => TransportBuilderEnum::WebTransportClient(WebTransportClientSocketBuilder {
                client_addr,
                server_addr,
            }),
            #[cfg(all(feature = "webtransport", target_family = "wasm"))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                certificate_digest,
            } => TransportBuilderEnum::WebTransportClient(WebTransportClientSocketBuilder {
                client_addr,
                server_addr,
                certificate_digest,
            }),
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportServer {
                server_addr,
                certificate,
            } => TransportBuilderEnum::WebTransportServer(WebTransportServerSocketBuilder {
                server_addr,
                certificate,
            }),
            #[cfg(feature = "websocket")]
            TransportConfig::WebSocketClient { server_addr } => {
                TransportBuilderEnum::WebSocketClient(WebSocketClientSocketBuilder { server_addr })
            }
            #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
            TransportConfig::WebSocketServer { server_addr } => {
                TransportBuilderEnum::WebSocketServer(WebSocketServerSocketBuilder { server_addr })
            }
            TransportConfig::Channels { channels } => {
                TransportBuilderEnum::Channels(Channels::new(channels))
            }
            TransportConfig::LocalChannel { recv, send } => {
                TransportBuilderEnum::LocalChannel(LocalChannelBuilder { recv, send })
            }
            TransportConfig::Dummy => TransportBuilderEnum::Dummy(DummyIo),
        }
    }
}

// TODO: derive Debug directly on TransportConfig once the new version of wtransport is out
impl Debug for TransportConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

#[derive(Clone, Debug, Reflect)]
#[reflect(from_reflect = false)]
pub struct IoConfig {
    #[reflect(ignore)]
    pub transport: TransportConfig,
    pub conditioner: Option<LinkConditionerConfig>,
}

impl Default for IoConfig {
    #[cfg(not(target_family = "wasm"))]
    fn default() -> Self {
        Self {
            transport: TransportConfig::UdpSocket(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0)),
            conditioner: None,
        }
    }

    #[cfg(target_family = "wasm")]
    fn default() -> Self {
        let (send, recv) = crossbeam_channel::unbounded();
        Self {
            transport: TransportConfig::LocalChannel { recv, send },
            conditioner: None,
        }
    }
}

impl IoConfig {
    pub fn from_transport(transport: TransportConfig) -> Self {
        Self {
            transport,
            conditioner: None,
        }
    }
    pub fn with_conditioner(mut self, conditioner_config: LinkConditionerConfig) -> Self {
        self.conditioner = Some(conditioner_config);
        self
    }

    pub fn build(self) -> Io {
        let conditioner = self.conditioner.map(LinkConditioner::new);
        let transport_builder = self.transport.build();
        Io::new(transport_builder, conditioner)
    }
}
