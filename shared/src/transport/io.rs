//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
#[cfg(feature = "metrics")]
use metrics;

use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::SocketAddr;

use crate::serialize::reader::ReadBuffer;
use crate::transport::conditioner::{ConditionedPacketReceiver, LinkConditionerConfig};
use crate::transport::{PacketReceiver, PacketSender, Transport};
use crate::UdpSocket;

pub struct Io {
    local_addr: SocketAddr,
    sender: Box<dyn PacketSender + Send + Sync>,
    receiver: Box<dyn PacketReceiver + Send + Sync>,
    // transport: Box<dyn Transport>,
    // read_buffer: ReadBuffer<'_>,
}

#[derive(Clone)]
pub enum TransportConfig {
    UdpSocket(SocketAddr),
}

#[derive(Clone)]
pub struct IoConfig {
    pub transport: TransportConfig,
    pub conditioner: Option<LinkConditionerConfig>,
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
}

impl Io {
    // pub(crate) fn new(transport: Box<dyn Transport>) -> Self {
    //     Self {
    //         transport,
    //         // read_buffer: ReadBuffer::new(),
    //     }
    // }

    pub fn from_config(config: &IoConfig) -> Result<Self> {
        match config.transport {
            TransportConfig::UdpSocket(ref addr) => {
                let socket = UdpSocket::new(addr)?;
                let local_addr = socket.local_addr();
                let sender = Box::new(socket.clone());

                let receiver: Box<dyn PacketReceiver + Send + Sync>;
                if let Some(conditioner) = &config.conditioner {
                    receiver = Box::new(ConditionedPacketReceiver::new(socket, conditioner));
                } else {
                    receiver = Box::new(socket);
                }
                Ok(Self::new(local_addr, sender, receiver))
            }
        }
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
            // transport,
            // read_buffer: ReadBuffer::new(),
        }
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
        // self.transport.local_addr()
    }
}
