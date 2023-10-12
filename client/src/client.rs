use std::collections::HashMap;

use anyhow::Context;

use lightyear_shared::netcode::ConnectToken;
use lightyear_shared::transport::{PacketReceiver, Transport};
use lightyear_shared::{ChannelKind, ChannelRegistry, Connection, Io, MessageContainer, Protocol};

use crate::io::ClientIO;

pub struct Client<P: Protocol> {
    io: Io,
    netcode: lightyear_shared::netcode::Client,
    message_manager: Connection<P>,
}

impl<P: Protocol> Client<P> {
    pub fn new(io: Io, token: ConnectToken, channel_registry: &'static ChannelRegistry) -> Self {
        // create netcode client from token
        let token_bytes = token
            .try_into_bytes()
            .expect("couldn't convert token to bytes");
        let netcode = lightyear_shared::netcode::Client::new(&token_bytes).unwrap();

        let message_manager = Connection::new(netcode.server_addr(), channel_registry);
        Self {
            io,
            netcode,
            message_manager,
        }
    }

    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.io.local_addr()
    }

    /// Start the connection process with the server
    pub fn connect(&mut self) {
        self.netcode.connect();
    }

    pub fn is_connected(&self) -> bool {
        self.netcode.is_connected()
    }

    /// Maintain connection with server, queues up any packet received from the server
    pub fn update(&mut self, time: f64) -> anyhow::Result<()> {
        self.netcode
            .try_update(time, &mut self.io)
            .context("Error updating netcode client")
    }

    /// Send a message to the server
    pub fn buffer_send(
        &mut self,
        message: MessageContainer<P::Message>,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        self.message_manager.buffer_send(message, channel_kind)
    }

    /// Receive messages from the server
    /// TODO: maybe use events?
    pub fn read_messages(&mut self) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
        self.message_manager.read_messages()
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        let mut client_io = ClientIO {
            io: &mut self.io,
            netcode: &mut self.netcode,
        };
        self.message_manager.send_packets(&mut client_io)
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> anyhow::Result<()> {
        loop {
            match self.netcode.recv() {
                Some(mut reader) => {
                    self.message_manager.recv_packet(&mut reader)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}
