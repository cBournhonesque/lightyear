//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use anyhow::Result;
use std::fmt::{Debug, Formatter};
use std::io;
use std::net::SocketAddr;

use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::transport::{PacketReader, PacketReceiver, PacketSender, Transport};

pub struct Io {
    transport: Box<dyn Transport>,
    // read_buffer: ReadBuffer<'_>,
}

impl Debug for Io {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish()
    }
}

impl PacketReceiver for Io {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        // todo: compression + bandwidth monitoring
        self.transport.recv()
    }
}

impl PacketSender for Io {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        // todo: compression + bandwidth monitoring
        self.transport.send(payload, address)
    }
}

impl PacketReader for Io {
    fn read<T: ReadBuffer>(&mut self) -> Result<Option<(T, SocketAddr)>> {
        Ok(self
            .recv()?
            .map(|(buffer, addr)| (T::start_read(buffer), addr)))
    }
}

impl Transport for Io {
    fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }
}
