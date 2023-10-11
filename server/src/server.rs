use lightyear_shared::{ChannelKind, Connection, Io, MessageContainer, Protocol};

use crate::io::ServerIO;
use anyhow::Context;
use lightyear_shared::netcode::ClientIndex;
use std::collections::HashMap;

pub struct Server<P: Protocol> {
    // Config

    // Clients
    io: Io,
    netcode: lightyear_shared::netcode::Server,
    user_connections: HashMap<ClientIndex, Connection<P>>,
}

impl<P: Protocol> Server<P> {
    /// Queues up a message to be sent to a client
    pub fn buffer_send(
        &mut self,
        client_id: ClientIndex,
        message: MessageContainer<P::Message>,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        self.user_connections
            .get_mut(&client_id)
            .context("client not found")?
            .buffer_send(message, channel_kind)
    }

    /// Update the server's internal state, queues up in a buffer any packets received from clients
    /// Sends keep-alive packets + any non-payload packet needed for netcode
    pub fn update(&mut self, time: f64) -> anyhow::Result<()> {
        let io = &mut self.io;
        self.netcode
            .try_update(time, io)
            .context("Error updating netcode server")
    }

    /// Receive messages from the server
    /// TODO: maybe use events?
    // pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
    //     self.message_manager.read_messages()
    // }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        let mut server_io = ServerIO {
            io: &mut self.io,
            netcode: &mut self.netcode,
        };
        for connection in &mut self.user_connections.values_mut() {
            connection.send_packets(&mut server_io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> anyhow::Result<()> {
        loop {
            match self.netcode.recv() {
                Some((mut reader, client_id)) => {
                    self.user_connections
                        .get_mut(&client_id)
                        .context("client not found")?
                        .recv_packet(&mut reader)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}
