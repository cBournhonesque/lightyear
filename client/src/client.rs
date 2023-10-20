use anyhow::Context;
use anyhow::Result;
use bevy_ecs::prelude::{Resource, World};
use std::net::SocketAddr;
use tracing::{debug, trace};

use lightyear_shared::netcode::{generate_key, Client as NetcodeClient};
use lightyear_shared::netcode::{ConnectToken, Key};
use lightyear_shared::transport::{PacketReceiver, PacketSender, Transport};
use lightyear_shared::{Channel, ChannelKind, Connection, Events, WriteBuffer};
use lightyear_shared::{Io, Protocol};

use crate::config::ClientConfig;

#[derive(Resource)]
pub struct Client<P: Protocol> {
    io: Io,
    protocol: P,
    netcode: lightyear_shared::netcode::Client,
    connection: Connection<P>,
}

pub enum Authentication {
    Token(ConnectToken),
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
}

impl Authentication {
    fn get_token(self) -> Option<ConnectToken> {
        match self {
            Authentication::Token(token) => Some(token),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .generate()
                .ok(),
        }
    }
}

impl<P: Protocol> Client<P> {
    pub fn new(config: ClientConfig, auth: Authentication, protocol: P) -> Self {
        let token = auth.get_token().expect("could not generate token");
        let token_bytes = token.try_into_bytes().unwrap();
        let netcode = NetcodeClient::with_config(&token_bytes, config.netcode.build())
            .expect("could not create netcode client");
        let io = Io::from_config(config.io).expect("could not build io");

        let connection = Connection::new(protocol.channel_registry());
        Self {
            io,
            protocol,
            netcode,
            connection,
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
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
    pub fn update(&mut self, time: f64) -> Result<()> {
        self.netcode
            .try_update(time, &mut self.io)
            .context("Error updating netcode client")
    }

    /// Send a message to the server
    pub fn buffer_send<C: Channel>(&mut self, message: P::Message) -> Result<()> {
        let channel = ChannelKind::of::<C>();
        self.connection.buffer_message(message, channel)
    }

    /// Receive messages from the server
    pub fn receive(&mut self, world: &mut World) -> Events<P> {
        trace!("Receive server packets");
        self.connection.receive(world)
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let packet_bytes = self.connection.send_packets()?;
        for mut packet_byte in packet_bytes {
            self.netcode
                .send(packet_byte.finish_write(), &mut self.io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> Result<()> {
        while let Some(mut reader) = self.netcode.recv() {
            self.connection.recv_packet(&mut reader)?;
        }
        Ok(())
    }
}
