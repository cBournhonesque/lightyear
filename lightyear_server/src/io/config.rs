use super::*;
use crate::prelude::CompressionConfig;
use crate::server::io::transport::{ServerTransportBuilder, ServerTransportBuilderEnum};
use crate::transport::channels::Channels;
use crate::transport::config::SharedIoConfig;
use crate::transport::dummy::DummyIo;
use crate::transport::io::IoStats;
#[cfg(feature = "zstd")]
use crate::transport::middleware::compression::zstd::compression::ZstdCompressor;
#[cfg(feature = "zstd")]
use crate::transport::middleware::compression::zstd::decompression::ZstdDecompressor;
use crate::transport::middleware::conditioner::LinkConditioner;
use crate::transport::middleware::PacketReceiverWrapper;
#[cfg(all(feature = "udp", not(target_family = "wasm")))]
use {
    core::net::IpAddr,
    crate::transport::udp::UdpSocketBuilder
};
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::WebSocketServerSocketBuilder;
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use crate::transport::webtransport::server::WebTransportServerSocketBuilder;
use crate::transport::BoxedReceiver;
use crate::transport::Transport;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec::Vec};
use bevy::prelude::TypePath;
use core::net::{SocketAddr};
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use wtransport::Identity;

#[derive(Debug, TypePath)]
pub enum ServerTransport {
    /// Use a [`UdpSocket`](std::net::UdpSocket)
    #[cfg(all(feature = "udp", not(target_family = "wasm")))]
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
        channels: Vec<(
            SocketAddr,
            crossbeam_channel::Receiver<Vec<u8>>,
            Sender<Vec<u8>>,
        )>,
    },
    /// Dummy transport if the connection handles its own io (for example steam sockets)
    Dummy,
}

/// We provide a manual implementation because wtranport's `Identity` does not implement Clone
impl Clone for ServerTransport {
    #[inline]
    fn clone(&self) -> ServerTransport {
        match self {
            #[cfg(all(feature = "udp", not(target_family = "wasm")))]
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
            #[cfg(all(feature = "udp", not(target_family = "wasm")))]
            ServerTransport::UdpSocket(addr) => {
                ServerTransportBuilderEnum::UdpSocket(UdpSocketBuilder { local_addr: addr })
            }
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            ServerTransport::WebTransportServer {
                server_addr,
                certificate,
            } => ServerTransportBuilderEnum::WebTransportServer(WebTransportServerSocketBuilder {
                server_addr,
                certificate,
            }),
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

impl Default for ServerTransport {

    #[cfg(all(feature = "udp", not(target_family = "wasm")))]
    fn default() -> Self {
        ServerTransport::UdpSocket(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0))
    }
    #[cfg(all(not(feature = "udp"), not(target_family = "wasm")))]
    fn default() -> Self {
        ServerTransport::Dummy
    }

    #[cfg(target_family = "wasm")]
    fn default() -> Self {
        ServerTransport::Dummy
    }
}

impl SharedIoConfig<ServerTransport> {
    pub fn start(self) -> Result<Io> {
        let (transport, state, io_rx, network_tx) = self.transport.build().start()?;
        let local_addr = transport.local_addr();
        #[allow(unused_mut)]
        let (mut sender, receiver) = transport.split();
        #[allow(unused_mut)]
        let mut receiver: BoxedReceiver = match self.conditioner { Some(conditioner_config) => {
            let conditioner = LinkConditioner::new(conditioner_config);
            Box::new(conditioner.wrap(receiver))
        } _ => {
            Box::new(receiver)
        }};
        match self.compression {
            CompressionConfig::None => {}
            #[cfg(feature = "zstd")]
            CompressionConfig::Zstd { level } => {
                use crate::transport::middleware::PacketSenderWrapper;
                let compressor = ZstdCompressor::new(level);
                sender = Box::new(compressor.wrap(sender));
                let decompressor = ZstdDecompressor::new();
                receiver = Box::new(decompressor.wrap(receiver));
            }
            #[cfg(feature = "lz4")]
            CompressionConfig::Lz4 => {
                use crate::transport::middleware::PacketSenderWrapper;
                let compressor =
                    crate::transport::middleware::compression::lz4::Compressor::default();
                sender = Box::new(compressor.wrap(sender));
                let decompressor =
                    crate::transport::middleware::compression::lz4::Decompressor::default();
                receiver = Box::new(decompressor.wrap(receiver));
            }
        }
        Ok(BaseIo {
            local_addr,
            sender,
            receiver,
            state,
            stats: IoStats::default(),
            context: IoContext {
                event_sender: network_tx,
                event_receiver: io_rx,
            },
        })
    }
}
