use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bevy::prelude::{Resource, Time, Virtual, World};
use tracing::trace;

use crate::inputs::input_buffer::InputBuffer;
use crate::netcode::Client as NetcodeClient;
use crate::netcode::{ConnectToken, Key};
use crate::tick::{Tick, TickManaged};
use crate::transport::{PacketReceiver, PacketSender, Transport};
use crate::{
    Channel, ChannelKind, ConnectionEvents, Message, SyncMessage, TickManager, TimeManager,
};
use crate::{Io, Protocol};

use super::config::ClientConfig;
use super::connection::Connection;

#[derive(Resource)]
pub struct Client<P: Protocol> {
    // Io
    io: Io,
    //config
    config: ClientConfig,
    // netcode
    netcode: crate::netcode::Client,
    // connection
    connection: Connection<P>,
    // protocol
    protocol: P,
    // events
    events: ConnectionEvents<P>,
    // syncing
    pub(crate) time_manager: TimeManager,
    tick_manager: TickManager,
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
        let config_clone = config.clone();
        let token = auth.get_token().expect("could not generate token");
        let token_bytes = token.try_into_bytes().unwrap();
        let netcode = NetcodeClient::with_config(&token_bytes, config.netcode.build())
            .expect("could not create netcode client");
        let io = Io::from_config(&config.io).expect("could not build io");

        let connection = Connection::new(protocol.channel_registry(), config.sync);
        Self {
            io,
            config: config_clone,
            protocol,
            netcode,
            connection,
            events: ConnectionEvents::new(),
            time_manager: TimeManager::new(config.packet.packet_send_interval),
            tick_manager: TickManager::from_config(config.shared.tick),
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

    /// Returns true if the client is connected and has been time-synced with the server
    pub fn is_synced(&self) -> bool {
        self.connection.sync_manager.is_synced()
    }

    // INPUT

    // TODO: maybe put the input_buffer directly in Client ?
    //  layer of indirection feelds annoying
    pub fn add_input(&mut self, input: P::Input) {
        self.connection
            .add_input(input, self.tick_manager.current_tick());
    }

    pub fn get_input_buffer(&self) -> &InputBuffer<P::Input> {
        &self.connection.input_buffer
    }

    /// Get a cloned version of the input (we might not want to pop from the buffer because we want
    /// to keep it for rollback)
    pub fn get_input(&mut self, tick: Tick) -> Option<P::Input> {
        self.connection.input_buffer.buffer.get(&tick).cloned()
    }

    // TIME

    pub fn is_ready_to_send(&self) -> bool {
        self.time_manager.is_ready_to_send()
    }

    pub fn set_base_relative_speed(&mut self, relative_speed: f32) {
        self.time_manager.base_relative_speed = relative_speed;
    }

    pub(crate) fn update_relative_speed(&mut self, time: &mut Time<Virtual>) {
        // check if we need to set the relative speed to something else
        if self.connection.sync_manager.is_synced() {
            self.connection
                .sync_manager
                .update_client_time(&mut self.time_manager, &self.tick_manager);
            // update bevy's relative speed
            self.time_manager.update_relative_speed(time);
            // let relative_speed = time.relative_speed();
            // info!( relative_speed = ?time.relative_speed(), "client virtual speed");
        }
    }

    // TICK

    pub fn tick(&self) -> Tick {
        self.tick_manager.current_tick()
    }

    pub fn latest_received_server_tick(&self) -> Tick {
        self.connection.sync_manager.latest_received_server_tick
    }

    pub fn received_new_server_tick(&self) -> bool {
        self.connection
            .sync_manager
            .duration_since_latest_received_server_tick
            == Duration::default()
    }

    pub(crate) fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }

    // REPLICATION

    /// Maintain connection with server, queues up any packet received from the server
    pub fn update(&mut self, delta: Duration, overstep: Duration) -> Result<()> {
        self.time_manager.update(delta, overstep);
        // self.tick_manager.update(delta);
        self.netcode.try_update(delta.as_secs_f64(), &mut self.io)?;

        // only start the connection (sending messages, sending pings, starting sync, etc.)
        // once we are connected
        if self.netcode.is_connected() {
            self.connection
                .update(&self.time_manager, &self.tick_manager);
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
        // TODO: time_manager is actually not needed by the client... code smell
        let mut events = self.connection.base.receive(world, &self.time_manager);

        // handle any sync messages
        for sync in events.into_iter_syncs() {
            match sync {
                SyncMessage::Ping(ping) => {
                    // self.connection.buffer_pong(&self.time_manager, ping);
                }
                SyncMessage::Pong(_) => {}
                SyncMessage::TimeSyncPing(_) => {
                    panic!("only client sends time sync messages to server")
                }
                SyncMessage::TimeSyncPong(pong) => {
                    self.connection.sync_manager.process_pong(
                        &pong,
                        &mut self.time_manager,
                        &mut self.tick_manager,
                    );
                }
            }
        }

        events
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let packet_bytes = self.connection.base.send_packets(&self.tick_manager)?;
        for mut packet_byte in packet_bytes {
            self.netcode.send(packet_byte.as_slice(), &mut self.io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> Result<()> {
        while let Some(mut reader) = self.netcode.recv() {
            self.connection
                .recv_packet(&mut reader, &self.tick_manager)?;
        }
        Ok(())
    }
}

impl<P: Protocol> TickManaged for Client<P> {
    fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }
}

// TODO: make this only available for integration tests
impl<P: Protocol> Client<P> {
    pub fn io(&self) -> &Io {
        &self.io
    }

    pub fn set_latest_received_server_tick(&mut self, tick: Tick) {
        self.connection.sync_manager.latest_received_server_tick = tick;
        self.connection
            .sync_manager
            .duration_since_latest_received_server_tick = Duration::default();
    }

    pub fn duration_since_latest_received_server_tick(&self) -> Duration {
        self.connection
            .sync_manager
            .duration_since_latest_received_server_tick
    }

    pub fn set_synced(&mut self) {
        self.connection.sync_manager.synced = true;
    }
}

// Functions related to Interpolation (maybe make it a trait)?
impl<P: Protocol> Client<P> {
    // TODO: how to mock this in tests?
    // TODO: actually we shouldn't use interpolation ticks, but use times directly, so we can take into account the overstep properly?
    pub(crate) fn interpolated_tick(&mut self) -> Tick {
        self.connection
            .sync_manager
            .update_estimated_interpolated_tick(
                &self.config.interpolation.delay,
                &self.tick_manager,
                &self.time_manager,
            );
        self.connection.sync_manager.estimated_interpolation_tick
    }
}
