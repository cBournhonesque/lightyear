use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use bevy::prelude::{Resource, World};
use tracing::trace;

use lightyear_shared::netcode::Client as NetcodeClient;
use lightyear_shared::netcode::{ConnectToken, Key};
use lightyear_shared::transport::{PacketReceiver, PacketSender, Transport};
use lightyear_shared::{
    Channel, ChannelKind, ConnectionEvents, Message, PingMessage, TimeManager, WriteBuffer,
};
use lightyear_shared::{Io, Protocol};

use crate::config::ClientConfig;
use crate::connection::Connection;

#[derive(Resource)]
pub struct Client<P: Protocol> {
    // Io
    io: Io,
    // netcode
    netcode: lightyear_shared::netcode::Client,
    // connection
    connection: Connection<P>,
    // protocol
    protocol: P,
    // events
    events: ConnectionEvents<P>,
    // syncing
    synced: bool,
    time_manager: TimeManager,
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
        let io = Io::from_config(&config.io).expect("could not build io");

        let connection = Connection::new(protocol.channel_registry(), &config.ping);
        Self {
            io,
            protocol,
            netcode,
            connection,
            events: ConnectionEvents::new(),
            synced: false,
            time_manager: TimeManager::new(),
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
    pub fn update(&mut self, delta: Duration) -> Result<()> {
        self.netcode.try_update(delta.as_secs_f64(), &mut self.io)?;

        // TODO: if is_connected but not time-synced, do a time-sync.
        //  exchange pings to compute RTT and match the ticks

        self.connection.update(delta);

        // TODO: run this only on client
        // complete tick syncing (only on client)?
        if !self.synced {
            // let ping = PingMessage::new()
            // self.buffer_message()
        }
        Ok(())
    }

    /// Send a message to the server
    pub fn buffer_send<C: Channel, M: Message>(&mut self, message: M) -> Result<()>
    where
        P::Message: From<M>,
    {
        let channel = ChannelKind::of::<C>();
        self.connection.base.buffer_message(message.into(), channel)
    }

    /// Receive messages from the server
    pub fn receive(&mut self, world: &mut World) -> ConnectionEvents<P> {
        trace!("Receive server packets");
        let mut events = self.connection.base.receive(world);

        // handle pings
        for ping in events.into_iter_pings() {
            self.connection.buffer_pong(&self.time_manager, ping);
        }
        // handle pongs
        for pong in events.into_iter_pongs() {
            // process pong to compute rtt/jitter and update ping store
            self.connection
                .ping_manager
                .process_pong(&pong, &self.time_manager);

            // process pong to update sync?
        }

        events
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let packet_bytes = self.connection.base.send_packets()?;
        for mut packet_byte in packet_bytes {
            self.netcode.send(packet_byte.as_slice(), &mut self.io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> Result<()> {
        while let Some(mut reader) = self.netcode.recv() {
            self.connection.base.recv_packet(&mut reader)?;
        }
        Ok(())
    }
}
