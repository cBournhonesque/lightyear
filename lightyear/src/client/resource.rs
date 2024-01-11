//! Defines the client bevy resource
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::{Entity, Resource, World};
use bevy::utils::EntityHashMap;
use tracing::{debug, trace, trace_span};

use crate::_reexport::ReplicationSend;
use crate::channel::builder::Channel;
use crate::connection::events::ConnectionEvents;
use crate::inputs::native::input_buffer::InputBuffer;
use crate::netcode::{Client as NetcodeClient, ClientId};
use crate::netcode::{ConnectToken, Key};
use crate::packet::message::Message;
use crate::prelude::NetworkTarget;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::receive::ReplicationReceiver;
use crate::shared::replication::send::ReplicationSender;
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
    pub(crate) connection: Connection<P>,
    // protocol
    protocol: P,
    // events
    events: ConnectionEvents<P>,
    // syncing
    pub(crate) time_manager: TimeManager,
    pub(crate) tick_manager: TickManager,
}

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

        let connection = Connection::new(
            protocol.channel_registry(),
            config.sync,
            &config.ping,
            config.prediction.input_delay_ticks,
        );
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

    // NETCODE

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

    /// Returns the client id assigned by the server
    pub fn id(&self) -> ClientId {
        self.netcode.id()
    }

    // IO

    pub fn io(&self) -> &Io {
        &self.io
    }

    // INPUT

    // TIME

    pub fn is_ready_to_send(&self) -> bool {
        self.time_manager.is_ready_to_send()
    }

    pub fn set_base_relative_speed(&mut self, relative_speed: f32) {
        self.time_manager.base_relative_speed = relative_speed;
    }

    // TICK

    pub fn latest_received_server_tick(&self) -> Tick {
        self.connection
            .sync_manager
            .latest_received_server_tick
            .unwrap_or(Tick(0))
    }

    pub fn received_new_server_tick(&self) -> bool {
        self.connection
            .sync_manager
            .duration_since_latest_received_server_tick
            == Duration::default()
    }

    // REPLICATION
    pub(crate) fn replication_sender(&self) -> &ReplicationSender<P> {
        &self.connection.replication_sender
    }

    pub(crate) fn replication_receiver(&self) -> &ReplicationReceiver<P> {
        &self.connection.replication_receiver
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
    /// - server prediction time is computed from time, which has been updated via delta
    /// Also server sends the tick after FixedUpdate, so it makes sense that we would compare to the client tick after FixedUpdate
    /// So instead we update the sync manager at PostUpdate, after both ticks/time have been updated
    pub(crate) fn sync_update(&mut self) {
        if self.netcode.is_connected() {
            self.connection.sync_manager.update(
                &mut self.time_manager,
                &mut self.tick_manager,
                &self.connection.ping_manager,
                &self.config.interpolation.delay,
                self.config.shared.server_send_interval,
            );

            if self.is_synced() {
                self.connection.sync_manager.update_prediction_time(
                    &mut self.time_manager,
                    &mut self.tick_manager,
                    &self.connection.ping_manager,
                );
            }
        }
    }

    // TODO: i'm not event sure that is something we want.
    //  it could open the door to the client flooding other players with messages
    //  maybe we should let users re-broadcast messages from the server themselves instead of using this
    //  Also it would make the code much simpler by having a single `ProtocolMessage` enum
    //  instead of `ClientMessage` and `ServerMessage`
    /// Send a message to the server, the message should be re-broadcasted according to the `target`
    pub fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) -> Result<()>
    where
        P::Message: From<M>,
    {
        let channel = ChannelKind::of::<C>();
        self.connection
            .buffer_message(message.into(), channel, target)
    }

    /// Send a message to the server
    pub fn send_message<C: Channel, M: Message>(&mut self, message: M) -> Result<()>
    where
        P::Message: From<M>,
    {
        let channel = ChannelKind::of::<C>();
        self.connection
            .buffer_message(message.into(), channel, NetworkTarget::None)
    }

    /// Receive messages from the server
    pub(crate) fn receive(&mut self, world: &mut World) -> ConnectionEvents<P> {
        trace!("Receive server packets");
        self.connection
            .receive(world, &self.time_manager, &self.tick_manager)
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub(crate) fn send_packets(&mut self) -> Result<()> {
        let packet_bytes = self
            .connection
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
                .recv_packet(&mut reader, &self.tick_manager)?;
        }
        Ok(())
    }
}

// INPUT
impl<P: Protocol> Client<P> {
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
}

impl<P: Protocol> TickManaged for Client<P> {
    fn tick(&self) -> Tick {
        self.tick_manager.current_tick()
    }

    fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }
}

// Access some internals for tests
#[cfg(test)]
impl<P: Protocol> Client<P> {
    pub fn set_latest_received_server_tick(&mut self, tick: Tick) {
        self.connection.sync_manager.latest_received_server_tick = Some(tick);
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

impl<P: Protocol> ReplicationSend<P> for Client<P> {
    fn new_connected_clients(&self) -> Vec<ClientId> {
        vec![]
    }

    fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group = replicate.group_id(Some(entity));
        trace!(?entity, "Send entity spawn for tick {:?}", self.tick());
        let replication_sender = &mut self.connection.replication_sender;
        // update the collect changes tick
        replication_sender
            .group_channels
            .entry(group)
            .or_default()
            .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_entity_spawn(entity, group);
        // Prediction/interpolation
        Ok(())
    }

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group = replicate.group_id(Some(entity));
        trace!(?entity, "Send entity despawn for tick {:?}", self.tick());
        let replication_sender = &mut self.connection.replication_sender;
        // update the collect changes tick
        replication_sender
            .group_channels
            .entry(group)
            .or_default()
            .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_entity_despawn(entity, group);
        // Prediction/interpolation
        Ok(())
    }

    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        let group = replicate.group_id(Some(entity));
        debug!(
            ?entity,
            component = ?kind,
            tick = ?self.tick_manager.current_tick(),
            "Inserting single component"
        );
        let replication_sender = &mut self.connection.replication_sender;
        // update the collect changes tick
        replication_sender
            .group_channels
            .entry(group)
            .or_default()
            .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_component_insert(entity, group, component.clone());
        Ok(())
    }

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        debug!(?entity, ?component_kind, "Sending RemoveComponent");
        let group = replicate.group_id(Some(entity));
        let replication_sender = &mut self.connection.replication_sender;
        replication_sender
            .group_channels
            .entry(group)
            .or_default()
            .update_collect_changes_since_this_tick(system_current_tick);
        replication_sender.prepare_component_remove(entity, group, component_kind);
        Ok(())
    }

    fn prepare_entity_update(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        let group = replicate.group_id(Some(entity));
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        let replication_sender = &mut self.connection.replication_sender;
        let collect_changes_since_this_tick = replication_sender
            .group_channels
            .entry(group)
            .or_default()
            .collect_changes_since_this_tick;
        // send the update for all changes newer than the last ack bevy tick for the group

        if collect_changes_since_this_tick.map_or(true, |c| {
            component_change_tick.is_newer_than(c, system_current_tick)
        }) {
            trace!(
                change_tick = ?component_change_tick,
                ?collect_changes_since_this_tick,
                current_tick = ?system_current_tick,
                "prepare entity update changed check"
            );
            trace!(
                ?entity,
                component = ?kind,
                tick = ?self.tick_manager.current_tick(),
                "Updating single component"
            );
            replication_sender.prepare_entity_update(entity, group, component.clone());
        }
        Ok(())
    }

    fn buffer_replication_messages(&mut self, bevy_tick: BevyTick) -> Result<()> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.connection
            .buffer_replication_messages(self.tick_manager.current_tick(), bevy_tick)
    }
    fn get_mut_replicate_component_cache(&mut self) -> &mut EntityHashMap<Entity, Replicate<P>> {
        &mut self.connection.replication_sender.replicate_component_cache
    }
}

// Functions related to Interpolation (maybe make it a trait)?
impl<P: Protocol> Client<P> {
    pub fn interpolation_tick(&self) -> Tick {
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
