//! Defines the client bevy resource
use bevy::utils::Duration;
use std::net::SocketAddr;

use anyhow::Result;
use bevy::prelude::{Resource, Time, Virtual, World};
use tracing::{debug, trace};

use crate::channel::builder::Channel;
use crate::connection::events::ConnectionEvents;
use crate::inputs::input_buffer::InputBuffer;
use crate::netcode::Client as NetcodeClient;
use crate::netcode::{ConnectToken, Key};
use crate::packet::message::Message;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::ping::message::SyncMessage;
use crate::shared::replication::manager::ReplicationManager;
use crate::shared::tick_manager::TickManager;
use crate::shared::tick_manager::{Tick, TickManaged};
use crate::shared::time_manager::TimeManager;
use crate::transport::io::Io;
use crate::transport::{PacketReceiver, PacketSender, Transport};

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

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
/// Struct used to authenticate with the server
pub enum Authentication {
    /// Use a `ConnectToken`
    Token(ConnectToken),
    /// Or build a `ConnectToken` manually from the given parameters
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
}

impl Authentication {
    fn get_token(self, client_timeout_secs: i32) -> Option<ConnectToken> {
        match self {
            Authentication::Token(token) => Some(token),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .timeout_seconds(client_timeout_secs)
                .generate()
                .ok(),
        }
    }
}

impl<P: Protocol> Client<P> {
    pub fn new(config: ClientConfig, io: Io, auth: Authentication, protocol: P) -> Self {
        let config_clone = config.clone();
        let token = auth
            .get_token(config.netcode.client_timeout_secs)
            .expect("could not generate token");
        let token_bytes = token.try_into_bytes().unwrap();
        let netcode = NetcodeClient::with_config(&token_bytes, config.netcode.build())
            .expect("could not create netcode client");

        let connection = Connection::new(protocol.channel_registry(), config.sync, &config.ping);
        Self {
            io,
            config: config_clone,
            protocol,
            netcode,
            connection,
            events: ConnectionEvents::new(),
            time_manager: TimeManager::new(config.shared.client_send_interval),
            tick_manager: TickManager::from_config(config.shared.tick),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
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

    pub fn get_mut_input_buffer(&mut self) -> &mut InputBuffer<P::Input> {
        &mut self.connection.input_buffer
    }

    /// Get a cloned version of the input (we might not want to pop from the buffer because we want
    /// to keep it for rollback)
    pub fn get_input(&mut self, tick: Tick) -> Option<P::Input> {
        self.connection.input_buffer.get(tick).cloned()
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
            self.connection.sync_manager.update_prediction_time(
                &mut self.time_manager,
                &self.tick_manager,
                &self.connection.base.ping_manager,
            );
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

    // REPLICATION
    pub(crate) fn replication_manager(&self) -> &ReplicationManager<P> {
        &self.connection.base.replication_manager
    }

    /// Maintain connection with server, queues up any packet received from the server
    pub(crate) fn update(&mut self, delta: Duration, overstep: Duration) -> Result<()> {
        self.time_manager.update(delta, overstep);
        self.netcode.try_update(delta.as_secs_f64(), &mut self.io)?;

        // only start the connection (sending messages, sending pings, starting sync, etc.)
        // once we are connected
        if self.netcode.is_connected() {
            self.connection
                .update(&self.time_manager, &self.tick_manager);
        }

        Ok(())
    }

    /// Update the sync manager.
    /// We run this at PostUpdate because:
    /// - client prediction time is computed from ticks, which haven't been updated yet at PreUpdate
    /// - server prediction time is computed from time, which has been update via delta
    /// Also server sends the tick after FixedUpdate, so it makes sense that we would compare to the client tick after FixedUpdate
    /// So instead we update the sync manager at PostUpdate, after both ticks/time have been updated
    pub(crate) fn sync_update(&mut self) {
        if self.netcode.is_connected() {
            self.connection.sync_manager.update(
                &mut self.time_manager,
                &mut self.tick_manager,
                &self.connection.base.ping_manager,
                &self.config.interpolation.delay,
                self.config.shared.server_send_interval,
            );
        }
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
    pub(crate) fn receive(&mut self, world: &mut World) -> ConnectionEvents<P> {
        trace!("Receive server packets");
        self.connection.base.receive(world, &self.time_manager)
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub(crate) fn send_packets(&mut self) -> Result<()> {
        let packet_bytes = self
            .connection
            .base
            .send_packets(&self.time_manager, &self.tick_manager)?;
        for packet_byte in packet_bytes {
            self.netcode.send(packet_byte.as_slice(), &mut self.io)?;
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub(crate) fn recv_packets(&mut self) -> Result<()> {
        while let Some(mut reader) = self.netcode.recv() {
            self.connection
                .recv_packet(&mut reader, &self.time_manager, &self.tick_manager)?;
        }
        Ok(())
    }
}

impl<P: Protocol> TickManaged for Client<P> {
    fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }
}

// Access some internals for tests
#[cfg(test)]
impl<P: Protocol> Client<P> {
    pub fn set_latest_received_server_tick(&mut self, tick: Tick) {
        self.connection.sync_manager.latest_received_server_tick = tick;
        self.connection
            .sync_manager
            .duration_since_latest_received_server_tick = Duration::default();
    }

    pub fn connection(&self) -> &Connection<P> {
        &self.connection
    }

    pub fn set_synced(&mut self) {
        self.connection.sync_manager.synced = true;
    }
}

// Functions related to Interpolation (maybe make it a trait)?
impl<P: Protocol> Client<P> {
    pub(crate) fn interpolation_tick(&self) -> Tick {
        self.connection
            .sync_manager
            .interpolation_tick(&self.tick_manager)
    }
    // // TODO: how to mock this in tests?
    // // TODO: actually we shouldn't use interpolation ticks, but use times directly, so we can take into account the overstep properly?
    // pub(crate) fn interpolated_tick(&mut self) -> Tick {
    //     self.connection
    //         .sync_manager
    //         .update_estimated_interpolated_tick(
    //             &self.config.interpolation.delay,
    //             &self.tick_manager,
    //             &self.time_manager,
    //         );
    //     self.connection.sync_manager.estimated_interpolation_tick
    // }
}
