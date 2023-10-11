// use anyhow::{anyhow, bail, Context, Result};
// use lightyear_shared::transport::{PacketReader, PacketReceiver, PacketSender};
// use lightyear_shared::{Io, ReadBuffer, ReadWordBuffer};
// use renetcode::NetcodeError;
// use std::net::SocketAddr;
// use std::time::Duration;
use std::io::Result;

use lightyear_shared::transport::PacketSender;
use lightyear_shared::Io;
use std::net::SocketAddr;

/// Wrapper around using the netcode.io protocol with a given transport
pub struct ClientIO<'i, 'n> {
    pub(crate) io: &'i mut Io,
    pub(crate) netcode: &'n mut lightyear_shared::netcode::Client,
}

// impl ServerIO<'_, '_> {
//     fn new(io: &mut Io, server: &mut lightyear_shared::netcode::Server) -> Self {
//         Self { io, server }
//     }
// }

impl PacketSender for ClientIO<'_, '_> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        self.netcode
            .send(payload, &mut self.io)
            .map_err(|e| std::io::Error::other(e))
    }
}

// /// Wrapper around using the netcode.io protocol with a given transport
// pub struct ClientIO {
//     io: Io,
//     client: renetcode::NetcodeClient,
// }
//
// impl PacketSender for ClientIO {
//     fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
//         if address != &self.remote_addr() {
//             bail!("Address does not match server address");
//         }
//         let (server_addr, packet) = self.client.generate_payload_packet(payload)?;
//         self.io
//             .send(packet, &server_addr)
//             .context("error sending packet")
//     }
// }
//
// impl PacketReader for ClientIO {
//     /// Receive a packet from io. Return a buffer containing the bytes if it was a payload packet.
//     /// If nothing is returned, it was a packet used for internal netcode purposes
//     fn read<T: ReadBuffer>(&mut self) -> Result<Option<(T, SocketAddr)>> {
//         let server_addr = self.remote_addr();
//         match self.io.recv()? {
//             None => Ok(None),
//             Some((buffer, addr)) => {
//                 if addr != server_addr {
//                     bail!("Address does not match server address");
//                 }
//                 Ok(self.client.process_packet(buffer).map(|b| {
//                     let reader = T::start_read(b);
//                     (reader, addr)
//                 }))
//             }
//         }
//     }
// }
//
// impl ClientIO {
//     pub fn new(io: Io, client: renetcode::NetcodeClient) -> Self {
//         Self { io, client }
//     }
//
//     pub fn remote_addr(&self) -> SocketAddr {
//         self.client.server_addr()
//     }
//
//     // TODO: SHOULD SEND REDUNDANT DISCONNECT PACKETS
//     /// Disconnect the client from the server.
//     /// Returns a disconnect packet that should be sent to the server.
//     pub fn disconnect(&mut self) -> std::io::Result<()> {
//         let (server_addr, packet) = self.client.disconnect()?;
//         self.io.send(packet, &server_addr)
//     }
//
//     /// Update the internal state of the client, receives the duration since last updated.
//     /// Might return the server address and a protocol packet to be sent to the server.
//     pub fn update(&mut self, duration: Duration) -> Result<()> {
//         match self.client.update(duration) {
//             Some((packet, server_addr)) => self.io.send(packet, &server_addr)?,
//             None => (),
//         };
//         Ok(())
//     }
// }
