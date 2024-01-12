//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use bevy::app::{App, Plugin};
use bevy::diagnostic::{Diagnostic, DiagnosticId, Diagnostics, RegisterDiagnostic};
use bevy::prelude::{Real, Res, Time};
use crossbeam_channel::{Receiver, Sender};
use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::{IpAddr, SocketAddr};

#[cfg(feature = "metrics")]
use metrics;
use tracing::info;

use super::LOCAL_SOCKET;
use crate::transport::channels::Channels;
use crate::transport::conditioner::{ConditionedPacketReceiver, LinkConditionerConfig};
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
    Channels {
        channels: Vec<(SocketAddr, Receiver<Vec<u8>>, Sender<Vec<u8>>)>,
    },
    LocalChannel {
        recv: Receiver<Vec<u8>>,
        send: Sender<Vec<u8>>,
    },
}

impl TransportConfig {
    pub fn get_io(self) -> Io {
        // we don't use `dyn Transport` and instead repeat the code for `transport.listen()` because that function is not
        // object-safe (we would get "the size of `dyn Transport` cannot be statically determined")
        match self {
            #[cfg(not(target_family = "wasm"))]
            TransportConfig::UdpSocket(addr) => {
                let transport = UdpSocket::new(addr).unwrap();
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
            } => {
                let transport = WebTransportClientSocket::new(client_addr, server_addr);
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
            #[cfg(all(feature = "webtransport", target_family = "wasm"))]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                certificate_digest,
            } => {
                let transport =
                    WebTransportClientSocket::new(client_addr, server_addr, certificate_digest);
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            TransportConfig::WebTransportServer {
                server_addr,
                certificate,
            } => {
                let transport = WebTransportServerSocket::new(server_addr, certificate);
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
            TransportConfig::Channels { channels } => {
                let mut transport = Channels::new();
                for (addr, remote_recv, remote_send) in channels.into_iter() {
                    transport.add_new_remote(addr, remote_recv, remote_send);
                }
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
            TransportConfig::LocalChannel { recv, send } => {
                let transport = LocalChannel::new(recv, send);
                let addr = transport.local_addr();
                let (sender, receiver) = transport.listen();
                Io::new(addr, sender, receiver)
            }
        }
    }
}

#[derive(Clone)]
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

    pub fn get_io(self) -> Io {
        let mut io = self.transport.get_io();
        if let Some(conditioner) = self.conditioner {
            io = Io::new(
                io.local_addr,
                io.sender,
                Box::new(ConditionedPacketReceiver::new(io.receiver, conditioner)),
            );
        }
        io
    }
}

pub struct Io {
    local_addr: SocketAddr,
    sender: Box<dyn PacketSender>,
    receiver: Box<dyn PacketReceiver>,
    pub(crate) stats: IoStats,
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
        local_addr: SocketAddr,
        sender: Box<dyn PacketSender>,
        receiver: Box<dyn PacketReceiver>,
    ) -> Self {
        Self {
            local_addr,
            sender,
            receiver,
            stats: IoStats::default(),
        }
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn to_parts(self) -> (Box<dyn PacketReceiver>, Box<dyn PacketSender>) {
        (self.receiver, self.sender)
    }

    pub fn split(&mut self) -> (&mut Box<dyn PacketSender>, &mut Box<dyn PacketReceiver>) {
        (&mut self.sender, &mut self.receiver)
    }

    pub fn stats(&self) -> &IoStats {
        &self.stats
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
        // TODO: INSPECT IS UNSTABLE

        self.receiver.recv().map(|x| {
            if let Some((ref buffer, _)) = x {
                #[cfg(feature = "metrics")]
                {
                    metrics::increment_counter!("transport.packets_received");
                    metrics::increment_gauge!("transport.bytes_received", buffer.len() as f64);
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
            metrics::increment_counter!("transport.packets_sent");
            metrics::increment_gauge!("transport.bytes_sent", payload.len() as f64);
        }
        self.stats.bytes_sent += payload.len();
        self.stats.packets_sent += 1;
        self.sender.send(payload, address)
    }
}

#[derive(Default)]
pub struct IoDiagnosticsPlugin;

impl IoDiagnosticsPlugin {
    /// How many bytes do we receive per second
    pub const BYTES_IN: DiagnosticId =
        DiagnosticId::from_u128(272724337309910272967747412065116587937);
    /// How many bytes do we send per second
    pub const BYTES_OUT: DiagnosticId =
        DiagnosticId::from_u128(55304262539591435450305383702521958293);

    /// How many bytes do we receive per second
    pub const PACKETS_IN: DiagnosticId =
        DiagnosticId::from_u128(183580771279032958450263611989577449811);
    /// How many bytes do we send per second
    pub const PACKETS_OUT: DiagnosticId =
        DiagnosticId::from_u128(314668465487051049643062180884137694217);

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
        diagnostics.add_measurement(Self::BYTES_IN, || {
            stats.bytes_received as f64 / delta_seconds
        });
        diagnostics.add_measurement(Self::BYTES_OUT, || stats.bytes_sent as f64 / delta_seconds);
        diagnostics.add_measurement(Self::PACKETS_IN, || {
            stats.packets_received as f64 / delta_seconds
        });
        diagnostics.add_measurement(Self::PACKETS_OUT, || {
            stats.packets_sent as f64 / delta_seconds
        });
        *stats = IoStats::default()
    }
}

impl Plugin for IoDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.register_diagnostic(Diagnostic::new(
            IoDiagnosticsPlugin::BYTES_IN,
            "bytes received per second",
            IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN,
        ));
        app.register_diagnostic(Diagnostic::new(
            IoDiagnosticsPlugin::BYTES_OUT,
            "bytes sent per second",
            IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN,
        ));
        app.register_diagnostic(Diagnostic::new(
            IoDiagnosticsPlugin::PACKETS_IN,
            "packets received per second",
            IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN,
        ));
        app.register_diagnostic(Diagnostic::new(
            IoDiagnosticsPlugin::PACKETS_OUT,
            "packets sent per second",
            IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN,
        ));
    }
}

impl PacketSender for Box<dyn PacketSender + Send + Sync> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }
}

impl PacketReceiver for Box<dyn PacketReceiver + Send + Sync> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        (**self).recv()
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
