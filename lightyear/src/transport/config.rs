use crate::prelude::{Io, LinkConditionerConfig};
use crate::transport::channels::Channels;
use crate::transport::dummy::DummyIo;
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
use {
    crate::transport::webtransport::server::{
        WebTransportServerSocket, WebTransportServerSocketBuilder,
    },
    wtransport::tls::Certificate,
};

use crate::transport::wrapper::conditioner::LinkConditioner;
use crate::transport::{Transport, TransportBuilderEnum};
use crossbeam_channel::{Receiver, Sender};
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, SocketAddr};

/// Use this to configure the [`Transport`] that will be used to establish a connection with the
/// remote.
#[derive(Clone)]
pub enum TransportConfig {
    #[cfg(not(target_family = "wasm"))]
    UdpSocket(SocketAddr),
    #[cfg(feature = "webtransport")]
    WebTransportClient {
        client_addr: SocketAddr,
        server_addr: SocketAddr,
        #[cfg(target_family = "wasm")]
        certificate_digest: String,
    },
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    WebTransportServer {
        server_addr: SocketAddr,
        certificate: Certificate,
    },
    #[cfg(feature = "websocket")]
    WebSocketClient { server_addr: SocketAddr },
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    WebSocketServer { server_addr: SocketAddr },
    Channels {
        channels: Vec<(SocketAddr, Receiver<Vec<u8>>, Sender<Vec<u8>>)>,
    },
    LocalChannel {
        recv: Receiver<Vec<u8>>,
        send: Sender<Vec<u8>>,
    },
    /// Dummy transport if the connection handles its own io (for example steamworks)
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

#[derive(Clone, Debug)]
pub struct IoConfig {
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
        let conditioner = self.conditioner.map(|config| LinkConditioner::new(config));
        let transport_builder = self.transport.build();
        Io::new(transport_builder, conditioner)
    }
}
