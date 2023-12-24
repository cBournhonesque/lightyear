//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::{IpAddr, SocketAddr};

#[cfg(feature = "metrics")]
use metrics;
#[cfg(feature = "webtransport")]
use wtransport::tls::Certificate;

use crate::transport::conditioner::{ConditionedPacketReceiver, LinkConditionerConfig};
use crate::transport::local::LocalChannel;
use crate::transport::udp::UdpSocket;
#[cfg(feature = "webtransport")]
use crate::transport::webtransport::client::WebTransportClientSocket;
#[cfg(feature = "webtransport")]
use crate::transport::webtransport::server::WebTransportServerSocket;
use crate::transport::{PacketReceiver, PacketSender, Transport};

#[derive(Clone)]
pub enum TransportConfig {
    UdpSocket(SocketAddr),
    #[cfg(feature = "webtransport")]
    WebTransportClient {
        client_addr: SocketAddr,
        server_addr: SocketAddr,
    },
    #[cfg(feature = "webtransport")]
    WebTransportServer {
        server_addr: SocketAddr,
        certificate: Certificate,
    },
    LocalChannel,
}

impl TransportConfig {
    pub fn get_io(&self) -> Io {
        let mut transport: Box<dyn Transport> = match self {
            TransportConfig::UdpSocket(addr) => Box::new(UdpSocket::new(addr).unwrap()),
            #[cfg(feature = "webtransport")]
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
            } => Box::new(WebTransportClientSocket::new(*client_addr, *server_addr)),
            #[cfg(feature = "webtransport")]
            TransportConfig::WebTransportServer {
                server_addr,
                certificate,
            } => Box::new(WebTransportServerSocket::new(
                *server_addr,
                certificate.clone(),
            )),
            TransportConfig::LocalChannel => Box::new(LocalChannel::new()),
        };
        let addr = transport.local_addr();
        let (sender, receiver) = transport.listen();
        Io::new(addr, sender, receiver)
    }
}

#[derive(Clone)]
pub struct IoConfig {
    pub transport: TransportConfig,
    pub conditioner: Option<LinkConditionerConfig>,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            transport: TransportConfig::UdpSocket(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0)),
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

    pub fn get_io(&self) -> Io {
        let mut io = self.transport.get_io();
        if let Some(conditioner) = &self.conditioner {
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
    stats: IoStats,
}

#[derive(Default, Debug)]
pub struct IoStats {
    pub bytes_sent: usize,
    pub bytes_received: usize,
}

impl Io {
    pub fn from_config(config: &IoConfig) -> Self {
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
        self.sender.send(payload, address)
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
