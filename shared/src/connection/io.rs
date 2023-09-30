//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use crate::packet::packet::MTU_PACKET_BYTES;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::transport::Transport;
use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::Buffer;
use std::net::SocketAddr;

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

    /// Returns a buffer that can be read from containing the data received from the transport
    pub fn create_reader_from_packet(
        &mut self,
    ) -> anyhow::Result<Option<(impl ReadBuffer, SocketAddr)>> {
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
