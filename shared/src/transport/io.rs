//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use std::fmt::{Debug, Formatter};
use std::io::Result;
use std::net::SocketAddr;

use crate::serialize::reader::ReadBuffer;
use crate::transport::{PacketReader, PacketReceiver, PacketSender, Transport};

pub struct Io {
    local_addr: SocketAddr,
    sender: Box<dyn PacketSender + Send>,
    receiver: Box<dyn PacketReceiver + Send>,
    // transport: Box<dyn Transport>,
    // read_buffer: ReadBuffer<'_>,
}

impl Io {
    // pub(crate) fn new(transport: Box<dyn Transport>) -> Self {
    //     Self {
    //         transport,
    //         // read_buffer: ReadBuffer::new(),
    //     }
    // }

    pub fn new(
        local_addr: SocketAddr,
        sender: Box<dyn PacketSender + Send>,
        receiver: Box<dyn PacketReceiver + Send>,
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
        &mut Box<dyn PacketSender + Send>,
        &mut Box<dyn PacketReceiver + Send>,
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
        self.receiver.recv()
    }
}

impl PacketSender for Io {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        // todo: compression + bandwidth monitoring
        self.sender.send(payload, address)
    }
}

impl PacketReader for Io {
    fn read<T: ReadBuffer>(&mut self) -> Result<Option<(T, SocketAddr)>> {
        Ok(self
            .recv()?
            .map(|(buffer, addr)| (T::start_read(buffer), addr)))
    }
}

impl PacketSender for Box<dyn PacketSender + Send> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        (**self).send(payload, address)
    }
}

impl PacketReceiver for Box<dyn PacketReceiver + Send> {
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
