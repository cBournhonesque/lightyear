use anyhow::{anyhow, bail, Context, Result};
use lightyear_shared::transport::{PacketReader, PacketReceiver, PacketSender};
use lightyear_shared::{Io, ReadBuffer, ReadWordBuffer};
use renetcode::NetcodeError;
use std::net::SocketAddr;
use std::time::Duration;

/// Wrapper around using the netcode.io protocol with a given transport
pub struct ServerIO {
    io: Io,
    server: renetcode::NetcodeServer,
}

impl PacketSender for ServerIO {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        if address != &self.remote_addr() {
            bail!("Address does not match server address");
        }
        // TODO: MIGHT WANT TO FORK RENETCODE TO DEAL WITH CLIENT IDS MYSELF?
        let client_id = self.server.
        let (server_addr, packet) = self.server.generate_payload_packet(payload)?;
        self.io
            .send(packet, &server_addr)
            .context("error sending packet")
    }
}

impl PacketReader for ServerIO {
    /// Receive a packet from io. Return a buffer containing the bytes if it was a payload packet.
    /// If nothing is returned, it was a packet used for internal netcode purposes
    fn read<T: ReadBuffer>(&mut self) -> Result<Option<(T, SocketAddr)>> {
        match self.io.recv()? {
            None => Ok(None),
            Some((buffer, addr)) => {
                Ok(self.server.process_packet(addr, buffer).map(|b| {
                    let reader = T::start_read(b);
                    (reader, addr)
                }))
            }
        }
    }
}

impl ClientIO {
    pub fn new(io: Io, client: renetcode::NetcodeClient) -> Self {
        Self { io, client }
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.client.server_addr()
    }

    // TODO: SHOULD SEND REDUNDANT DISCONNECT PACKETS
    /// Disconnect the client from the server.
    /// Returns a disconnect packet that should be sent to the server.
    pub fn disconnect(&mut self) -> Result<()> {
        let (server_addr, packet) = self.client.disconnect()?;
        self.io.send(packet, &server_addr)
    }

    /// Update the internal state of the client, receives the duration since last updated.
    /// Might return the server address and a protocol packet to be sent to the server.
    pub fn update(&mut self, duration: Duration) -> Result<()> {
        match self.client.update(duration) {
            Some((packet, server_addr)) => self.io.send(packet, &server_addr)?,
            None => (),
        };
        Ok(())
    }
}
