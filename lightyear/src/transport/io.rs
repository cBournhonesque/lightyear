//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::{IpAddr, SocketAddr};

#[cfg(feature = "metrics")]
use metrics;

use crate::transport::conditioner::{ConditionedPacketReceiver, LinkConditionerConfig};
use crate::transport::local::{LocalChannel, LOCAL_SOCKET};
use crate::transport::udp::UdpSocket;
use crate::transport::{PacketReceiver, PacketSender, Transport};

#[derive(Clone)]
pub enum TransportConfig {
    UdpSocket(SocketAddr),
    LocalChannel,
}

impl TransportConfig {
    pub fn get_io(&self) -> Io {
        match self {
            TransportConfig::UdpSocket(addr) => {
                let socket = UdpSocket::new(addr).unwrap();
                Io::new(*addr, Box::new(socket.clone()), Box::new(socket))
            }
            TransportConfig::LocalChannel => {
                let channel = LocalChannel::new();
                Io::new(LOCAL_SOCKET, Box::new(channel.clone()), Box::new(channel))
            }
        }
    }
    // pub fn get_local_addr(&self) -> SocketAddr {
    //     match self {
    //         TransportConfig::UdpSocket(addr) => *addr,
    //         TransportConfig::LocalChannel => LOCAL_SOCKET,
    //     }
    // }
    // pub fn get_sender(&self) -> Box<dyn PacketSender + Send + Sync> {
    //     match self {
    //         TransportConfig::UdpSocket(addr) => Box::new(UdpSocket::new(addr).unwrap()),
    //         TransportConfig::LocalChannel => Box::new(LocalChannel::new()),
    //     }
    // }
    //
    // pub fn get_receiver(&self) -> Box<dyn PacketReceiver + Send + Sync> {
    //     match self {
    //         TransportConfig::UdpSocket(addr) => Box::new(UdpSocket::new(addr).unwrap()),
    //         TransportConfig::LocalChannel => Box::new(LocalChannel::new()),
    //     }
    // }
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

    // pub fn get_local_addr(&self) -> SocketAddr {
    //     self.transport.get_local_addr()
    // }
    // pub fn get_sender(&self) -> Box<dyn PacketSender + Send + Sync> {
    //     self.transport.get_sender()
    // }
    //
    // pub fn get_receiver(&self) -> Box<dyn PacketReceiver + Send + Sync> {
    //     let mut receiver = self.transport.get_receiver();
    //     if let Some(conditioner) = &self.conditioner {
    //         receiver = Box::new(ConditionedPacketReceiver::new(receiver, conditioner));
    //     }
    //     receiver
    // }
}

pub struct Io {
    local_addr: SocketAddr,
    sender: Box<dyn PacketSender + Send + Sync>,
    receiver: Box<dyn PacketReceiver + Send + Sync>,
}

impl Io {
    pub fn from_config(config: &IoConfig) -> Self {
        config.get_io()
        // let local_addr = config.transport.get_local_addr();
        // let sender = config.transport.get_sender();
        // let receiver = config.transport.get_receiver();
        // Self::new(local_addr, sender, receiver)
    }

    pub fn new(
        local_addr: SocketAddr,
        sender: Box<dyn PacketSender + Send + Sync>,
        receiver: Box<dyn PacketReceiver + Send + Sync>,
    ) -> Self {
        Self {
            local_addr,
            sender,
            receiver,
        }
    }

    pub fn to_parts(
        self,
    ) -> (
        Box<dyn PacketReceiver + Send + Sync>,
        Box<dyn PacketSender + Send + Sync>,
    ) {
        (self.receiver, self.sender)
    }

    pub fn split(
        &mut self,
    ) -> (
        &mut Box<dyn PacketSender + Send + Sync>,
        &mut Box<dyn PacketReceiver + Send + Sync>,
    ) {
        (&mut self.sender, &mut self.receiver)
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

impl Transport for Io {
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}
