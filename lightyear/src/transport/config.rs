use bevy::prelude::Reflect;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, SocketAddr};

use crossbeam_channel::{Receiver, Sender};

#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use {
    crate::transport::webtransport::server::WebTransportServerSocketBuilder,
    wtransport::tls::Identity,
};

use crate::prelude::Io;
use crate::transport::channels::Channels;
use crate::transport::client::ClientTransportBuilderEnum;
use crate::transport::dummy::DummyIo;
use crate::transport::error::Result;
use crate::transport::io::IoStats;
use crate::transport::local::LocalChannelBuilder;
#[cfg(feature = "zstd")]
use crate::transport::middleware::compression::zstd::{
    compression::ZstdCompressor, decompression::ZstdDecompressor,
};
use crate::transport::middleware::compression::CompressionConfig;
use crate::transport::middleware::conditioner::{LinkConditioner, LinkConditionerConfig};
use crate::transport::middleware::{PacketReceiverWrapper, PacketSenderWrapper};
#[cfg(not(target_family = "wasm"))]
use crate::transport::udp::UdpSocketBuilder;
#[cfg(feature = "websocket")]
use crate::transport::websocket::client::WebSocketClientSocketBuilder;
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::WebSocketServerSocketBuilder;
#[cfg(feature = "webtransport")]
use crate::transport::webtransport::client::WebTransportClientSocketBuilder;
use crate::transport::{BoxedReceiver, Transport};

mod client {
    use super::*;
    use crate::transport::channels::Channels;
    use crate::transport::client::ClientTransportBuilderEnum;
    use crate::transport::local::LocalChannelBuilder;
    use crate::transport::websocket::client::WebSocketClientSocketBuilder;
    use crate::transport::webtransport::client::WebTransportClientSocketBuilder;

    /// Use this to configure the [`Transport`] that will be used to establish a connection with the
    /// server.
    #[derive(Clone, Debug)]
    pub enum ClientTransport {
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
        /// Use [`WebSocket`](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket) as a transport
        #[cfg(feature = "websocket")]
        WebSocketClient { server_addr: SocketAddr },
        /// Use a crossbeam_channel as a transport. This is useful for testing.
        /// This is mostly for clients.
        LocalChannel {
            recv: Receiver<Vec<u8>>,
            send: Sender<Vec<u8>>,
        },
        /// Dummy transport if the connection handles its own io (for example steam sockets)
        Dummy,
    }

    impl ClientTransport {
        fn build(self) -> ClientTransportBuilderEnum {
            match self {
                #[cfg(not(target_family = "wasm"))]
                ClientTransport::UdpSocket(addr) => {
                    ClientTransportBuilderEnum::UdpSocket(UdpSocketBuilder { local_addr: addr })
                }
                #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
                ClientTransport::WebTransportClient {
                    client_addr,
                    server_addr,
                } => ClientTransportBuilderEnum::WebTransportClient(
                    WebTransportClientSocketBuilder {
                        client_addr,
                        server_addr,
                    },
                ),
                #[cfg(all(feature = "webtransport", target_family = "wasm"))]
                ClientTransport::WebTransportClient {
                    client_addr,
                    server_addr,
                    certificate_digest,
                } => TransportBuilderEnum::WebTransportClient(WebTransportClientSocketBuilder {
                    client_addr,
                    server_addr,
                    certificate_digest,
                }),
                #[cfg(feature = "websocket")]
                ClientTransport::WebSocketClient { server_addr } => {
                    ClientTransportBuilderEnum::WebSocketClient(WebSocketClientSocketBuilder {
                        server_addr,
                    })
                }
                ClientTransport::LocalChannel { recv, send } => {
                    ClientTransportBuilderEnum::LocalChannel(LocalChannelBuilder { recv, send })
                }
                ClientTransport::Dummy => ClientTransportBuilderEnum::Dummy(DummyIo),
            }
        }
    }

    impl Default for ClientTransport {
        #[cfg(not(target_family = "wasm"))]
        fn default() -> Self {
            ClientTransport::UdpSocket(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0)),
        }

        #[cfg(target_family = "wasm")]
        fn default() -> Self {
            let (send, recv) = crossbeam_channel::unbounded();
            ClientTransport::LocalChannel { recv, send },
        }
    }
}

mod server {
    use super::*;
    use crate::transport::local::LocalChannelBuilder;
    use crate::transport::server::ServerTransportBuilderEnum;
    use crate::transport::websocket::server::WebSocketServerSocketBuilder;
    use crate::transport::webtransport::server::WebTransportServerSocketBuilder;
    use wtransport::Identity;

    #[derive(Debug)]
    pub enum ServerTransport {
        /// Use a [`UdpSocket`](std::net::UdpSocket)
        #[cfg(not(target_family = "wasm"))]
        UdpSocket(SocketAddr),
        /// Use [`WebTransport`](https://wicg.github.io/web-transport/) as a transport layer
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        WebTransportServer {
            server_addr: SocketAddr,
            /// Certificate that will be used for authentication
            certificate: Identity,
        },
        /// Use [`WebSocket`](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket) as a transport
        #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
        WebSocketServer { server_addr: SocketAddr },
        /// Use a crossbeam_channel as a transport. This is useful for testing.
        /// This is server-only: each tuple corresponds to a different client.
        Channels {
            channels: Vec<(SocketAddr, Receiver<Vec<u8>>, Sender<Vec<u8>>)>,
        },
        /// Dummy transport if the connection handles its own io (for example steam sockets)
        Dummy,
    }

    /// We provide a manual implementation because wtranport's `Identity` does not implement Clone
    impl Clone for ServerTransport {
        #[inline]
        fn clone(&self) -> ServerTransport {
            match self {
                #[cfg(not(target_family = "wasm"))]
                ServerTransport::UdpSocket(__self_0) => {
                    ServerTransport::UdpSocket(Clone::clone(__self_0))
                }
                #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
                ServerTransport::WebTransportServer {
                    server_addr: __self_0,
                    certificate: __self_1,
                } => ServerTransport::WebTransportServer {
                    server_addr: Clone::clone(__self_0),
                    certificate: __self_1.clone_identity(),
                },
                #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
                ServerTransport::WebSocketServer {
                    server_addr: __self_0,
                } => ServerTransport::WebSocketServer {
                    server_addr: Clone::clone(__self_0),
                },
                ServerTransport::Channels { channels: __self_0 } => ServerTransport::Channels {
                    channels: Clone::clone(__self_0),
                },
                ServerTransport::Dummy => ServerTransport::Dummy,
            }
        }
    }

    impl ServerTransport {
        fn build(self) -> ServerTransportBuilderEnum {
            match self {
                #[cfg(not(target_family = "wasm"))]
                ServerTransport::UdpSocket(addr) => {
                    ServerTransportBuilderEnum::UdpSocket(UdpSocketBuilder { local_addr: addr })
                }
                #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
                ServerTransport::WebTransportServer {
                    server_addr,
                    certificate,
                } => ServerTransportBuilderEnum::WebTransportServer(
                    WebTransportServerSocketBuilder {
                        server_addr,
                        certificate,
                    },
                ),
                #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
                ServerTransport::WebSocketServer { server_addr } => {
                    ServerTransportBuilderEnum::WebSocketServer(WebSocketServerSocketBuilder {
                        server_addr,
                    })
                }
                ServerTransport::Channels { channels } => {
                    ServerTransportBuilderEnum::Channels(Channels::new(channels))
                }
                ServerTransport::Dummy => ServerTransportBuilderEnum::Dummy(DummyIo),
            }
        }
    }
}

#[derive(Clone, Debug, Reflect)]
#[reflect(from_reflect = false)]
pub struct IoConfig<T> {
    #[reflect(ignore)]
    pub transport: T,
    pub conditioner: Option<LinkConditionerConfig>,
    pub compression: CompressionConfig,
}

impl Default for IoConfig {
    #[cfg(not(target_family = "wasm"))]
    fn default() -> Self {
        Self {
            transport: ClientTransport::UdpSocket(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0)),
            conditioner: None,
            compression: CompressionConfig::default(),
        }
    }

    #[cfg(target_family = "wasm")]
    fn default() -> Self {
        let (send, recv) = crossbeam_channel::unbounded();
        Self {
            transport: ClientTransport::LocalChannel { recv, send },
            conditioner: None,
            compression: CompressionConfig::default(),
        }
    }
}

impl IoConfig {
    pub fn from_transport(transport: ClientTransport) -> Self {
        Self {
            transport,
            conditioner: None,
            compression: CompressionConfig::default(),
        }
    }
    pub fn with_conditioner(mut self, conditioner_config: LinkConditionerConfig) -> Self {
        self.conditioner = Some(conditioner_config);
        self
    }

    pub fn with_compression(mut self, compression_config: CompressionConfig) -> Self {
        self.compression = compression_config;
        self
    }

    pub fn connect(self) -> Result<Io> {
        let (transport, state, event_receiver) = self.transport.build().connect()?;
        let local_addr = transport.local_addr();
        #[allow(unused_mut)]
        let (mut sender, receiver, close_fn) = transport.split();
        #[allow(unused_mut)]
        let mut receiver: BoxedReceiver = if let Some(conditioner_config) = self.conditioner {
            let conditioner = LinkConditioner::new(conditioner_config);
            Box::new(conditioner.wrap(receiver))
        } else {
            Box::new(receiver)
        };
        match self.compression {
            CompressionConfig::None => {}
            #[cfg(feature = "zstd")]
            CompressionConfig::Zstd { level } => {
                let compressor = ZstdCompressor::new(level);
                sender = Box::new(compressor.wrap(sender));
                let decompressor = ZstdDecompressor::new();
                receiver = Box::new(decompressor.wrap(receiver));
            }
        }
        Ok(Io {
            local_addr,
            sender,
            receiver,
            close_fn,
            state,
            event_receiver,
            stats: IoStats::default(),
        })
    }
}
