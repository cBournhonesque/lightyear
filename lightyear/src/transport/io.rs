//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use bevy::app::{App, Plugin};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::prelude::{Real, Res, Resource, Time};
use crossbeam_channel::{Receiver, Sender};
use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::{IpAddr, SocketAddr};

use crate::_reexport::ComponentBehaviour;
#[cfg(feature = "metrics")]
use metrics;
use tracing::info;

use super::LOCAL_SOCKET;
use crate::transport::channels::Channels;
use crate::transport::conditioner::{ConditionedPacketReceiver, LinkConditionerConfig};
use crate::transport::dummy::DummyIo;
use crate::transport::local::LocalChannel;
use crate::transport::{PacketReceiver, PacketSender, Transport};

#[cfg(not(target_family = "wasm"))]
use crate::transport::udp::UdpSocket;

cfg_if::cfg_if! {
    if #[cfg(all(feature = "webtransport", not(target_family = "wasm")))] {
        use wtransport::tls::Certificate;
        use crate::transport::webtransport::server::WebTransportServerSocket;
    }
}

#[cfg(feature = "webtransport")]
use crate::transport::webtransport::client::WebTransportClientSocket;

#[cfg(feature = "websocket")]
use crate::transport::websocket::client::WebSocketClientSocket;
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::WebSocketServerSocket;

#[derive(Clone)]
pub enum TransportConfig {
    // TODO: should we have a features for UDP?
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

// TODO: derive Debug directly on TransportConfig once the new version of wtransport is out
impl Debug for TransportConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl TransportConfig {
    pub fn build_transport(self) -> Box<dyn Transport> {
        // we don't use `dyn Transport` and instead repeat the code for `transport.listen()` because that function is not
        // object-safe (we would get "the size of `dyn Transport` cannot be statically determined")
        match self {
            #[cfg_attr(docsrs, doc(cfg(not(target_family = "wasm"))))]
            #[cfg(not(target_family = "wasm"))]
            TransportConfig::UdpSocket(addr) => {
                let transport = UdpSocket::new(addr).unwrap();
                Box::new(transport)
            }
            #[cfg_attr(
                docsrs,
                doc(cfg(all(feature = "webtransport", not(target_family = "wasm"))))
            )]
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
            } => {
                let transport = WebTransportClientSocket::new(client_addr, server_addr);
                Box::new(transport)
            }
            #[cfg_attr(
                docsrs,
                doc(cfg(all(feature = "webtransport", target_family = "wasm")))
            )]
            #[cfg(all(feature = "webtransport", target_family = "wasm"))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                certificate_digest,
            } => {
                let transport =
                    WebTransportClientSocket::new(client_addr, server_addr, certificate_digest);
                let addr = transport.local_addr();
                Io::new(Box::new(transport), addr)
            }
            #[cfg_attr(
                docsrs,
                doc(cfg(all(feature = "webtransport", not(target_family = "wasm"))))
            )]
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportServer {
                server_addr,
                certificate,
            } => {
                let transport = WebTransportServerSocket::new(server_addr, certificate);
                Box::new(transport)
            }
            #[cfg_attr(docsrs, doc(cfg(feature = "websocket")))]
            #[cfg(feature = "websocket")]
            TransportConfig::WebSocketClient { server_addr } => {
                let transport = WebSocketClientSocket::new(server_addr);
                Box::new(transport)
            }
            #[cfg_attr(
                docsrs,
                doc(cfg(all(feature = "websocket", not(target_family = "wasm"))))
            )]
            #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
            TransportConfig::WebSocketServer { server_addr } => {
                let transport = WebSocketServerSocket::new(server_addr);
                Box::new(transport)
            }
            TransportConfig::Channels { channels } => {
                let mut transport = Channels::new();
                for (addr, remote_recv, remote_send) in channels.into_iter() {
                    transport.add_new_remote(addr, remote_recv, remote_send);
                }
                Box::new(transport)
            }
            TransportConfig::LocalChannel { recv, send } => {
                let transport = LocalChannel::new(recv, send);
                Box::new(transport)
            }
            TransportConfig::Dummy => {
                let transport = DummyIo;
                Box::new(transport)
            }
        }
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
        Self {
            transport: TransportConfig::Dummy,
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

    pub fn get_io(self) -> Io {
        let transport = self.transport.build_transport();
        Io::new(transport, self.conditioner)
    }
}

#[derive(Resource)]
pub struct Io {
    transport: Option<Box<dyn Transport>>,
    local_addr: SocketAddr,
    sender: Option<Box<dyn PacketSender>>,
    receiver: Option<Box<dyn PacketReceiver>>,
    conditioner_config: Option<LinkConditionerConfig>,
    pub(crate) stats: IoStats,
}

impl Default for Io {
    fn default() -> Self {
        panic!("Io::default() is not implemented. Please provide an io");
    }
}

#[derive(Default, Debug)]
pub struct IoStats {
    pub bytes_sent: usize,
    pub bytes_received: usize,
    pub packets_sent: usize,
    pub packets_received: usize,
}

impl Io {
    pub fn from_config(config: IoConfig) -> Self {
        config.get_io()
    }

    pub fn new(
        transport: Box<dyn Transport>,
        conditioner_config: Option<LinkConditionerConfig>,
    ) -> Self {
        Self {
            transport: Some(transport),
            local_addr: transport.local_addr(),
            sender: None,
            receiver: None,
            conditioner_config,
            stats: IoStats::default(),
        }
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn connect(&mut self) -> Result<()> {
        let (sender, receiver) = std::mem::take(&mut self.transport).connect()?;
        self.sender = Some(sender);
        if let Some(conditioner) = std::mem::take(&mut self.conditioner_config) {
            self.receiver = Some(Box::new(ConditionedPacketReceiver::new(
                receiver,
                conditioner,
            )));
        } else {
            self.receiver = Some(receiver);
        }
        Ok(())
    }

    pub fn split(&mut self) -> (&mut dyn PacketSender, &mut dyn PacketReceiver) {
        (
            self.sender.as_mut().unwrap(),
            self.receiver.as_mut().unwrap(),
        )
    }

    pub fn stats(&self) -> &IoStats {
        &self.stats
    }

    pub fn close(&mut self) -> Result<()> {
        self.sender.close()?;
        self.receiver.close()
    }
}

impl Debug for Io {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish()
    }
}

impl PacketReceiver for Io {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        // todo: compression + bandwidth monitoring
        self.receiver.as_mut().unwrap().recv().map(|x| {
            if let Some((ref buffer, _)) = x {
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("transport.packets_received").increment(1);
                    metrics::gauge!("transport.bytes_received").increment(buffer.len() as f64);
                }
                self.stats.bytes_received += buffer.len();
                self.stats.packets_received += 1;
            }
            x
        })
    }
}

impl PacketSender for Io {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        // todo: compression + bandwidth monitoring
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("transport.packets_sent").increment(1);
            metrics::gauge!("transport.bytes_sent").increment(payload.len() as f64);
        }
        self.stats.bytes_sent += payload.len();
        self.stats.packets_sent += 1;
        self.sender.as_mut().unwrap().send(payload, address)
    }
}

pub struct IoDiagnosticsPlugin;

impl IoDiagnosticsPlugin {
    /// How many bytes do we receive per second
    pub const BYTES_IN: DiagnosticPath = DiagnosticPath::const_new("KB received per second");
    /// How many bytes do we send per second
    pub const BYTES_OUT: DiagnosticPath = DiagnosticPath::const_new("KB sent per second");

    /// How many bytes do we receive per second
    pub const PACKETS_IN: DiagnosticPath = DiagnosticPath::const_new("packets received per second");
    /// How many bytes do we send per second
    pub const PACKETS_OUT: DiagnosticPath = DiagnosticPath::const_new("packets sent per second");

    /// Max diagnostic history length.
    pub const DIAGNOSTIC_HISTORY_LEN: usize = 60;

    pub(crate) fn update_diagnostics(
        stats: &mut IoStats,
        time: &Res<Time<Real>>,
        diagnostics: &mut Diagnostics,
    ) {
        let delta_seconds = time.delta_seconds_f64();
        if delta_seconds == 0.0 {
            return;
        }
        diagnostics.add_measurement(&Self::BYTES_IN, || {
            (stats.bytes_received as f64 / 1000.0) / delta_seconds
        });
        diagnostics.add_measurement(&Self::BYTES_OUT, || {
            (stats.bytes_sent as f64 / 1000.0) / delta_seconds
        });
        diagnostics.add_measurement(&Self::PACKETS_IN, || {
            stats.packets_received as f64 / delta_seconds
        });
        diagnostics.add_measurement(&Self::PACKETS_OUT, || {
            stats.packets_sent as f64 / delta_seconds
        });
        *stats = IoStats::default()
    }
}

impl Plugin for IoDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::BYTES_IN)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::BYTES_OUT)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::PACKETS_IN)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::PACKETS_OUT)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
    }
}

impl PacketSender for Box<dyn PacketSender + Send + Sync> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }

    fn close(&mut self) -> Result<()> {
        (**self).close()
    }
}

impl PacketReceiver for Box<dyn PacketReceiver + Send + Sync> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (**self).recv()
    }

    fn close(&mut self) -> Result<()> {
        (**self).close()
    }
}

// impl Transport for Io {
//     fn local_addr(&self) -> SocketAddr {
//         self.local_addr
//     }
//
//     fn listen(&mut self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
//         (self.sender.clone(), self.receiver.clone())
//     }
// }
