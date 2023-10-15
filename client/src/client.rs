use anyhow::Context;
use anyhow::Result;
use bevy_ecs::prelude::Resource;

use lightyear_shared::netcode::ConnectToken;
use lightyear_shared::transport::{PacketReceiver, PacketSender, Transport};
use lightyear_shared::{Channel, Connection, Events, WriteBuffer};
use lightyear_shared::{Io, Protocol};

#[derive(Resource)]
pub struct Client<P: Protocol> {
    io: Io,
    protocol: P,
    netcode: lightyear_shared::netcode::Client,
    connection: Connection<P>,
}

impl<P: Protocol> Client<P> {
    pub fn new(io: Io, token: ConnectToken, protocol: P) -> Self {
        // create netcode client from token
        let token_bytes = token
            .try_into_bytes()
            .expect("couldn't convert token to bytes");
        let netcode = lightyear_shared::netcode::Client::new(&token_bytes).unwrap();

        let connection = Connection::new(protocol.channel_registry());
        Self {
            io,
            protocol,
            netcode,
            connection,
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

    // REPLICATION

    /// Maintain connection with server, queues up any packet received from the server
    pub fn update(&mut self, time: f64) -> anyhow::Result<()> {
        self.netcode
            .try_update(time, &mut self.io)
            .context("Error updating netcode client")
    }

    /// Send a message to the server
    pub fn buffer_send<C: Channel>(&mut self, message: P::Message) -> Result<()> {
        self.connection.buffer_message::<C>(message)
    }

    /// Receive messages from the server
    pub fn receive(&mut self) -> Events<P> {
        self.connection.receive()
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        let packet_bytes = self.connection.send_packets()?;
        for mut packet_byte in packet_bytes {
            self.netcode
                .send(packet_byte.finish_write(), &mut self.io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> anyhow::Result<()> {
        loop {
            match self.netcode.recv() {
                Some(mut reader) => {
                    self.connection.recv_packet(&mut reader)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}
