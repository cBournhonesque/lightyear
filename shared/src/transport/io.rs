//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use std::io;
use std::io::Result;
use std::net::SocketAddr;

use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::transport::Transport;

pub struct Io {
    transport: Box<dyn Transport>,
    // read_buffer: ReadBuffer<'_>,
}

impl Io {
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self {
            transport,
            // read_buffer: ReadBuffer::new(),
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.transport.local_addr()
    }

    // TODO: use io.error?
    pub fn send_packet(&mut self, packet: &[u8], remote_addr: &SocketAddr) -> Result<()> {
        // Compression
        // Bandwidth monitoring
        self.transport.send(packet, remote_addr)
    }

    pub fn recv_packet(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        // TODO: Add Compression + Bandwidth monitoring
        self.transport.recv()
    }

    /// Returns a buffer that can be read from containing the data received from the transport
    pub fn create_reader_from_packet(&mut self) -> Result<Option<(impl ReadBuffer, SocketAddr)>> {
        match self.transport.recv()? {
            None => Ok(None),
            Some((data, addr)) => {
                // this copies the data into the buffer, so we can read efficiently from it
                // we can now re-use the transport's buffer.
                // maybe it would be safer to provide a buffer for the transport to use?
                let mut buffer = ReadWordBuffer::start_read(data);
                Ok(Some((buffer, addr)))
            }
        }
    }
}
