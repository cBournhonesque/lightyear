use std::net::SocketAddr;
use bevy_ecs::world::World;


#[cfg(feature = "bevy_support")]
use bevy_ecs::prelude::Resource;

#[cfg(feature = "webrtc")]
use naia_client_socket::Socket;
use tracing::warn;

pub use crate::shared::{
    serde::{BitReader, BitWriter, Serde},
    Channel, ChannelId, Channels, ConnectionConfig, EntityDoesNotExistError,
    Message, PacketType, PingConfig, PingIndex, Protocol,
    SocketConfig, StandardHeader, Tick, Timer, Timestamp,
};

use crate::client::{
    connection::{
        connection::Connection,
        handshake_manager::{HandshakeManager, HandshakeResult},
        io::Io,
    },
    tick::tick_manager::TickManager,
};

use super::{client_config::ClientConfig, error::NaiaClientError, events::Events};

/// Client can send/receive messages to/from a server, and has a pool of
/// in-scope entities/components that are synced with the server
#[cfg_attr(feature = "bevy_support", derive(Resource))]
pub struct Client {
    // Config
    client_config: ClientConfig,
    protocol: Protocol,
    // Connection
    io: Io,
    server_connection: Option<Connection>,
    handshake_manager: HandshakeManager,
    // Events
    incoming_events: Events,
    // Ticks
    tick_manager: Option<TickManager>,
}

impl Client {
    /// Create a new Client
    pub fn new(client_config: ClientConfig, protocol: Protocol) -> Self {
        let handshake_manager = HandshakeManager::new(client_config.send_handshake_interval);

        let tick_manager = protocol
            .tick_interval
            .map(|duration| TickManager::new(duration, client_config.minimum_latency));

        let compression_config = protocol.compression.clone();

        Client {
            // Config
            client_config: client_config.clone(),
            protocol,
            // Connection
            io: Io::new(
                &client_config.connection.bandwidth_measure_duration,
                &compression_config,
            ),
            server_connection: None,
            handshake_manager,
            // Events
            incoming_events: Events::new(),
            // Ticks
            tick_manager,
        }
    }

    /// Set the auth object to use when setting up a connection with the Server
    pub fn auth<M: Message>(&mut self, auth: M) {
        self.handshake_manager.set_auth_message(Box::new(auth));
    }

    /// Connect to the given server address
    pub fn connect(&mut self, server_session_url: &str) {
        if !self.is_disconnected() {
            panic!("Client has already initiated a connection, cannot initiate a new one. TIP: Check client.is_disconnected() before calling client.connect()");
        }
        let mut socket = Socket::new(&self.protocol.socket);
        socket.connect(server_session_url);
        self.io
            .load(socket.packet_sender(), socket.packet_receiver());
    }

    /// Returns whether or not the client is disconnected
    pub fn is_disconnected(&self) -> bool {
        !self.io.is_loaded()
    }

    /// Returns whether or not a connection is being established with the Server
    pub fn is_connecting(&self) -> bool {
        self.io.is_loaded()
    }

    /// Returns whether or not a connection has been established with the Server
    pub fn is_connected(&self) -> bool {
        self.server_connection.is_some()
    }

    /// Disconnect from Server
    pub fn disconnect(&mut self) {
        if !self.is_connected() {
            panic!("Trying to disconnect Client which is not connected yet!")
        }

        for _ in 0..10 {
            let mut writer = self.handshake_manager.write_disconnect();
            match self.io.send_writer(&mut writer) {
                Ok(()) => {}
                Err(_) => {
                    // TODO: pass this on and handle above
                    warn!("Client Error: Cannot send disconnect packet to Server");
                }
            }
        }

        self.disconnect_internal();
    }

    // Receive Data from Server! Very important!

    /// Must call this regularly (preferably at the beginning of every draw
    /// frame), in a loop until it returns None.
    /// Retrieves incoming update data from the server, and maintains the connection.
    pub fn receive(&mut self, mut world: World) -> Events {
        // Need to run this to maintain connection with server, and receive packets
        // until none left
        self.maintain_socket();

        // all other operations
        if let Some(server_connection) = self.server_connection.as_mut() {
            if server_connection.base.should_drop() {
                self.disconnect_internal();
                return std::mem::take(&mut self.incoming_events);
            }

            let mut did_tick = false;

            // update current tick
            if let Some(tick_manager) = &mut self.tick_manager {
                if tick_manager.recv_client_tick() {
                    did_tick = true;

                    // apply updates on tick boundary
                    let receiving_tick = tick_manager.client_receiving_tick();
                    server_connection.process_buffered_packets(
                        &mut world,
                        receiving_tick,
                        &mut self.incoming_events,
                    );
                }
            } else {
                server_connection.process_buffered_packets(
                    &mut world,
                    0,
                    &mut self.incoming_events,
                );
            }

            // receive (process) messages
            server_connection
                .base
                .message_manager
                .receive_messages(&mut self.incoming_events);

            // send outgoing packets
            server_connection.send_outgoing_packets(&mut self.io, &self.tick_manager);

            // tick event
            if did_tick {
                self.incoming_events.push_tick();
            }
        } else {
            self.handshake_manager.send(&mut self.io);
        }

        std::mem::take(&mut self.incoming_events)
    }

    // Messages

    /// Queues up an Message to be sent to the Server
    pub fn send_message<C: Channel, M: Message>(&mut self, message: &M) {
        let cloned_message = M::clone_box(message);
        self.send_message_inner(&Channels::type_to_id::<C>(), cloned_message);
    }

    fn send_message_inner(&mut self, channel_id: &ChannelId, message: Box<dyn Message>) {
        let channel_settings = Channels::channel(channel_id);
        if !channel_settings.can_send_to_server() {
            panic!("Cannot send message to Server on this Channel");
        }

        let tick_buffered = channel_settings.tick_buffered();

        if tick_buffered {
            if let Some(client_tick) = self.client_tick() {
                if let Some(connection) = self.server_connection.as_mut() {
                    connection
                        .tick_buffer
                        .as_mut()
                        .expect("connection does not have a tick buffer")
                        .send_message(&client_tick, channel_id, message);
                }
            }
        } else if let Some(connection) = &mut self.server_connection {
            connection
                .base
                .message_manager
                .send_message(channel_id, message);
        }
    }

    // Connection

    /// Get the address currently associated with the Server
    pub fn server_address(&self) -> Result<SocketAddr, NaiaClientError> {
        self.io.server_addr()
    }

    /// Gets the average Round Trip Time measured to the Server
    pub fn rtt(&self) -> f32 {
        self.server_connection
            .as_ref()
            .expect("it is expected that you should verify whether the client is connected before calling this method")
            .ping_manager.rtt
    }

    /// Gets the average Jitter measured in connection to the Server
    pub fn jitter(&self) -> f32 {
        self.server_connection
            .as_ref()
            .expect("it is expected that you should verify whether the client is connected before calling this method")
            .ping_manager.jitter
    }

    // Ticks

    /// Gets the current tick of the Client
    pub fn client_tick(&self) -> Option<Tick> {
        return self
            .tick_manager
            .as_ref()
            .map(|tick_manager| tick_manager.client_sending_tick());
    }

    // Interpolation

    /// Gets the interpolation tween amount for the current frame
    pub fn interpolation(&self) -> Option<f32> {
        self.tick_manager
            .as_ref()
            .map(|tick_manager| tick_manager.interpolation())
    }

    // Bandwidth monitoring

    pub fn outgoing_bandwidth(&mut self) -> f32 {
        self.io.outgoing_bandwidth()
    }

    pub fn incoming_bandwidth(&mut self) -> f32 {
        self.io.incoming_bandwidth()
    }

    // internal functions

    fn maintain_socket(&mut self) {
        // get current tick
        if let Some(server_connection) = self.server_connection.as_mut() {
            // connection already established

            // send heartbeats
            if server_connection.base.should_send_heartbeat() {
                let mut writer = BitWriter::new();

                // write header
                server_connection
                    .base
                    .write_outgoing_header(PacketType::Heartbeat, &mut writer);

                // write client tick
                if let Some(tick_manager) = self.tick_manager.as_mut() {
                    tick_manager.write_client_tick(&mut writer);
                }

                // send packet
                match self.io.send_writer(&mut writer) {
                    Ok(()) => {}
                    Err(_) => {
                        // TODO: pass this on and handle above
                        warn!("Client Error: Cannot send heartbeat packet to Server");
                    }
                }
                server_connection.base.mark_sent();
            }

            // send pings
            if server_connection.ping_manager.should_send_ping() {
                let mut writer = BitWriter::new();

                // write header
                server_connection
                    .base
                    .write_outgoing_header(PacketType::Ping, &mut writer);

                // write client tick
                if let Some(tick_manager) = self.tick_manager.as_mut() {
                    tick_manager.write_client_tick(&mut writer);
                }

                // write body
                server_connection.ping_manager.write_ping(&mut writer);

                // send packet
                match self.io.send_writer(&mut writer) {
                    Ok(()) => {}
                    Err(_) => {
                        // TODO: pass this on and handle above
                        warn!("Client Error: Cannot send ping packet to Server");
                    }
                }
                server_connection.base.mark_sent();
            }

            // receive from socket
            loop {
                match self.io.recv_reader() {
                    Ok(Some(mut reader)) => {
                        server_connection.base.mark_heard();

                        let header = StandardHeader::de(&mut reader)
                            .expect("unable to parse header from incoming packet");

                        match header.packet_type {
                            PacketType::Data
                            | PacketType::Heartbeat
                            | PacketType::Ping
                            | PacketType::Pong => {
                                // continue, these packet types are allowed when
                                // connection is established
                            }
                            _ => {
                                // short-circuit, do not need to handle other packet types at this
                                // point
                                continue;
                            }
                        }

                        // Read incoming header
                        server_connection.process_incoming_header(&header);

                        // Record incoming tick
                        let mut incoming_tick = 0;

                        if let Some(tick_manager) = self.tick_manager.as_mut() {
                            incoming_tick = tick_manager.read_server_tick(
                                &mut reader,
                                server_connection.ping_manager.rtt,
                                server_connection.ping_manager.jitter,
                            );
                        }

                        // Handle based on PacketType
                        match header.packet_type {
                            PacketType::Data => {
                                server_connection.buffer_data_packet(incoming_tick, &mut reader);
                            }
                            PacketType::Heartbeat => {
                                // already marked as heard, job done
                            }
                            PacketType::Ping => {
                                // read incoming ping index
                                let ping_index = PingIndex::de(&mut reader)
                                    .expect("unable to parse an index from Ping packet");

                                // write pong payload
                                let mut writer = BitWriter::new();

                                // write header
                                server_connection
                                    .base
                                    .write_outgoing_header(PacketType::Pong, &mut writer);

                                // write server tick
                                if let Some(tick_manager) = self.tick_manager.as_ref() {
                                    tick_manager.write_client_tick(&mut writer);
                                }

                                // write index
                                ping_index.ser(&mut writer);

                                // send packet
                                match self.io.send_writer(&mut writer) {
                                    Ok(()) => {}
                                    Err(_) => {
                                        // TODO: pass this on and handle above
                                        warn!("Client Error: Cannot send pong packet to Server");
                                    }
                                }
                                server_connection.base.mark_sent();
                            }
                            PacketType::Pong => {
                                server_connection.ping_manager.process_pong(&mut reader);
                            }
                            _ => {
                                // no other packet types matter when connection
                                // is established
                            }
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(error) => {
                        self.incoming_events
                            .push_error(NaiaClientError::Wrapped(Box::new(error)));
                    }
                }
            }
        } else {
            // No connection established yet
            if self.io.is_loaded() {
                // receive from socket
                loop {
                    match self.io.recv_reader() {
                        Ok(Some(mut reader)) => {
                            match self.handshake_manager.recv(&mut reader) {
                                Some(HandshakeResult::Connected) => {
                                    // new connect!
                                    let server_addr = self.server_address_unwrapped();
                                    self.server_connection = Some(Connection::new(
                                        server_addr,
                                        &self.client_config.connection,
                                        &self.protocol.tick_interval,
                                    ));
                                    self.incoming_events.push_connection(&server_addr);
                                }
                                Some(HandshakeResult::Rejected) => {
                                    let server_addr = self.server_address_unwrapped();
                                    self.incoming_events.clear();
                                    self.incoming_events.push_rejection(&server_addr);
                                    self.disconnect_cleanup();
                                    return;
                                }
                                None => {}
                            }
                        }
                        Ok(None) => {
                            break;
                        }
                        Err(error) => {
                            self.incoming_events
                                .push_error(NaiaClientError::Wrapped(Box::new(error)));
                        }
                    }
                }
            }
        }
    }

    fn disconnect_internal(&mut self) {
        let server_addr = self.server_address_unwrapped();
        self.disconnect_cleanup();

        // exit early, we're disconnected, who cares?
        self.incoming_events.clear();
        self.incoming_events.push_disconnection(&server_addr);
    }

    fn disconnect_cleanup(&mut self) {
        // this is very similar to the newtype method .. can we coalesce and reduce
        // duplication?
        let tick_manager = {
            if let Some(duration) = self.protocol.tick_interval {
                Some(TickManager::new(
                    duration,
                    self.client_config.minimum_latency,
                ))
            } else {
                None
            }
        };

        self.io = Io::new(
            &self.client_config.connection.bandwidth_measure_duration,
            &self.protocol.compression,
        );
        self.server_connection = None;
        self.handshake_manager = HandshakeManager::new(self.client_config.send_handshake_interval);
        self.tick_manager = tick_manager;
    }

    fn server_address_unwrapped(&self) -> SocketAddr {
        // NOTE: may panic if the connection is not yet established!
        self.io.server_addr().expect("connection not established!")
    }
}