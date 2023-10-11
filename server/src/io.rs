use anyhow::{anyhow, bail, Context};
use lightyear_shared::transport::{PacketReader, PacketReceiver, PacketSender};
use lightyear_shared::{Io, ReadBuffer, ReadWordBuffer};
use renetcode::NetcodeError;
use std::io::Result;
use std::net::SocketAddr;
use std::time::Duration;

/// Wrapper around using the netcode.io protocol with a given transport
pub struct ServerIO<'i, 'n> {
    pub(crate) io: &'i mut Io,
    pub(crate) netcode: &'n mut lightyear_shared::netcode::Server,
}

// impl ServerIO<'_, '_> {
//     fn new(io: &mut Io, server: &mut lightyear_shared::netcode::Server) -> Self {
//         Self { io, server }
//     }
// }

impl PacketSender for ServerIO<'_, '_> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        let client_index = self
            .netcode
            .client_index(address)
            .ok_or_else(|| std::io::Error::other("client not found"))?;
        self.netcode
            .send(payload, client_index, &mut self.io)
            .map_err(|e| std::io::Error::other(e))
    }
}
//
// impl PacketReader for ServerIO {
//     /// Receive a packet from io. Return a buffer containing the bytes if it was a payload packet.
//     /// If nothing is returned, it was a packet used for internal netcode purposes
//     fn read<T: ReadBuffer>(&mut self) -> Result<Option<(T, SocketAddr)>> {
//         match self.io.recv()? {
//             None => Ok(None),
//             Some((buffer, addr)) => {
//                 Ok(self.server.process_packet(addr, buffer).map(|b| {
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
//     pub fn disconnect(&mut self) -> Result<()> {
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
