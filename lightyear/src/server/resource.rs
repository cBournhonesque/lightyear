//! Defines the server bevy resource
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use bevy::ecs::component::Tick as BevyTick;
use bevy::prelude::{Entity, Resource, World};
use bevy::utils::HashSet;
use crossbeam_channel::Sender;
use tracing::{debug, debug_span, info, trace, trace_span};

use crate::channel::builder::Channel;
use crate::inputs::input_buffer::InputBuffer;
use crate::netcode::{generate_key, ClientId, ConnectToken};
use crate::packet::message::Message;
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::server::room::{RoomId, RoomManager, RoomMut, RoomRef};
use crate::shared::replication::components::{NetworkTarget, Replicate};
use crate::shared::replication::components::{ShouldBeInterpolated, ShouldBePredicted};
use crate::shared::replication::ReplicationSend;
use crate::shared::tick_manager::TickManager;
use crate::shared::tick_manager::{Tick, TickManaged};
use crate::shared::time_manager::TimeManager;
use crate::transport::io::Io;
use crate::transport::{PacketSender, Transport};

use super::config::ServerConfig;
use super::connection::Connection;
use super::events::ServerEvents;

#[derive(Resource)]
pub struct Server<P: Protocol> {
    // Config
    config: ServerConfig,
    // Io
    io: Io,
    // Netcode
    netcode: crate::netcode::Server<NetcodeServerContext>,
    context: ServerContext,
    // Clients
    user_connections: HashMap<ClientId, Connection<P>>,
    // TODO: maybe put this in replication plugin
    // list of clients that connected since the last time we sent replication messages
    // (we want to keep track of them because we need to replicate the entire world state to them)
    pub(crate) new_clients: Vec<ClientId>,
    // Protocol
    pub protocol: P,
    // Events
    pub(crate) events: ServerEvents<P>,
    // Rooms
    pub(crate) room_manager: RoomManager,
    // Time
    time_manager: TimeManager,
    tick_manager: TickManager,
}

pub struct NetcodeServerContext {
    pub connections: Sender<ClientId>,
    pub disconnections: Sender<ClientId>,
}

impl<P: Protocol> Server<P> {
    pub fn new(config: ServerConfig, io: Io, protocol: P) -> Self {
        // create netcode server
        let private_key = config.netcode.private_key.unwrap_or(generate_key());
        let (connections_tx, connections_rx) = crossbeam_channel::unbounded();
        let (disconnections_tx, disconnections_rx) = crossbeam_channel::unbounded();
        let server_context = NetcodeServerContext {
            connections: connections_tx,
            disconnections: disconnections_tx,
        };
        let mut cfg = crate::netcode::ServerConfig::with_context(server_context)
            .on_connect(|id, ctx| {
                ctx.connections.send(id).unwrap();
            })
            .on_disconnect(|id, ctx| {
                ctx.disconnections.send(id).unwrap();
            });
        cfg = cfg.keep_alive_send_rate(config.netcode.keep_alive_send_rate);
        cfg = cfg.num_disconnect_packets(config.netcode.num_disconnect_packets);

        let netcode =
            crate::netcode::Server::with_config(config.netcode.protocol_id, private_key, cfg)
                .expect("Could not create server netcode");
        let context = ServerContext {
            connections: connections_rx,
            disconnections: disconnections_rx,
        };
        Self {
            config: config.clone(),
            io,
            netcode,
            context,
            user_connections: HashMap::new(),
            new_clients: Vec::new(),
            protocol,
            events: ServerEvents::new(),
            room_manager: RoomManager::default(),
            time_manager: TimeManager::new(config.shared.server_send_interval),
            tick_manager: TickManager::from_config(config.shared.tick),
        }
    }

    /// Generate a connect token for a client with id `client_id`
    pub fn token(&mut self, client_id: ClientId) -> ConnectToken {
        info!("timeout: {:?}", self.config.netcode.client_timeout_secs);
        self.netcode
            .token(client_id, self.local_addr())
            .timeout_seconds(self.config.netcode.client_timeout_secs)
            .generate()
            .unwrap()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.io.local_addr()
    }

    pub fn client_ids(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.netcode.client_ids()
    }

    // EVENTS

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    // INPUTS

    // TODO: exposed only for debugging
    pub fn get_input_buffer(&self, client_id: ClientId) -> Option<&InputBuffer<P::Input>> {
        self.user_connections
            .get(&client_id)
            .map(|connection| &connection.input_buffer)
    }

    /// Get the inputs for all clients for the given tick
    pub fn pop_inputs(&mut self) -> impl Iterator<Item = (Option<P::Input>, ClientId)> + '_ {
        self.user_connections
            .iter_mut()
            .map(|(client_id, connection)| {
                let received_input = connection
                    .input_buffer
                    .pop(self.tick_manager.current_tick());
                let fallback = received_input.is_none();

                // NOTE: if there is no input for this tick, we should use the last input that we have
                //  as a best-effort fallback.
                let input = match received_input {
                    None => connection.last_input.clone(),
                    Some(i) => {
                        connection.last_input = Some(i.clone());
                        Some(i)
                    }
                };
                // let input = received_input.map_or_else(
                //     || connection.last_input.clone(),
                //     |i| {
                //         connection.last_input = Some(i.clone());
                //         Some(i)
                //     },
                // );
                if fallback {
                    // TODO: do not log this while clients are syncing..
                    debug!(
                        ?client_id,
                        tick = ?self.tick_manager.current_tick(),
                        fallback_input = ?&input,
                        "Missed client input!"
                    )
                }
                // TODO: We should also let the user know that it needs to send inputs a bit earlier so that
                //  we have more of a buffer. Send a SyncMessage to tell the user to speed up?
                //  See Overwatch GDC video
                (input, *client_id)
            })
    }

    // TIME

    #[doc(hidden)]
    pub fn is_ready_to_send(&self) -> bool {
        self.time_manager.is_ready_to_send()
    }

    #[doc(hidden)]
    pub fn set_base_relative_speed(&mut self, relative_speed: f32) {
        self.time_manager.base_relative_speed = relative_speed;
    }

    // TICK

    #[doc(hidden)]
    pub fn tick(&self) -> Tick {
        self.tick_manager.current_tick()
    }

    // REPLICATION
    /// Find the list of clients that should receive the replication message
    fn apply_replication(&mut self, target: NetworkTarget) -> Box<dyn Iterator<Item = ClientId>> {
        match target {
            NetworkTarget::All => {
                // TODO: maybe only send stuff when the client is time-synced ?
                Box::new(self.netcode.connected_client_ids().into_iter())
            }
            NetworkTarget::AllExcept(client_ids) => {
                let client_ids: HashSet<ClientId> = HashSet::from_iter(client_ids);
                Box::new(
                    self.netcode
                        .connected_client_ids()
                        .into_iter()
                        .filter(move |id| !client_ids.contains(id)),
                )
            }
            NetworkTarget::Only(client_ids) => Box::new(client_ids.into_iter()),
            NetworkTarget::None => Box::new(std::iter::empty()),
        }
    }

    // MESSAGES

    /// Queues up a message to be sent to all clients
    pub fn send_to_target<C: Channel, M: Message>(
        &mut self,
        message: M,
        target: NetworkTarget,
    ) -> Result<()>
    where
        M: Clone,
        P::Message: From<M>,
    {
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("send_message_server_before_span");
        }
        let _span = debug_span!("broadcast", user = "a").entered();
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("send_message_server_after_span");
        }
        let channel = ChannelKind::of::<C>();
        for client_id in self
            .netcode
            .connected_client_ids()
            .iter()
            .filter(|id| target.should_send_to(id))
        {
            self.user_connections
                .get_mut(client_id)
                .context("client not found")?
                .base
                .buffer_message(message.clone().into(), channel)?;
        }
        Ok(())
    }

    /// Queues up a message to be sent to a client
    pub fn buffer_send<C: Channel, M: Message>(
        &mut self,
        client_id: ClientId,
        message: M,
    ) -> Result<()>
    where
        P::Message: From<M>,
    {
        let _span = debug_span!("buffer_send", client_id = ?client_id).entered();
        let channel = ChannelKind::of::<C>();
        // TODO: if client not connected; buffer in advance?
        self.user_connections
            .get_mut(&client_id)
            .context("client not found")?
            .base
            .buffer_message(message.into(), channel)
    }

    /// Update the server's internal state, queues up in a buffer any packets received from clients
    /// Sends keep-alive packets + any non-payload packet needed for netcode
    pub fn update(&mut self, delta: Duration) -> Result<()> {
        // update time manager
        self.time_manager.update(delta, Duration::default());

        // update netcode server
        self.netcode
            .try_update(delta.as_secs_f64(), &mut self.io)
            .context("Error updating netcode server")?;

        // update connections
        for connection in self.user_connections.values_mut() {
            connection
                .base
                .update(&self.time_manager, &self.tick_manager);
        }

        // handle connections
        for client_id in self.context.connections.try_iter() {
            // TODO: do we need a mutex around this?
            if let Entry::Vacant(e) = self.user_connections.entry(client_id) {
                #[cfg(feature = "metrics")]
                metrics::increment_gauge!("connected_clients", 1.0);

                let client_addr = self.netcode.client_addr(client_id).unwrap();
                info!("New connection from {} (id: {})", client_addr, client_id);
                let mut connection =
                    Connection::new(self.protocol.channel_registry(), &self.config.ping);
                connection.base.events.push_connection();
                self.new_clients.push(client_id);
                e.insert(connection);
            }
        }

        // handle disconnections
        for client_id in self.context.disconnections.try_iter() {
            #[cfg(feature = "metrics")]
            metrics::decrement_gauge!("connected_clients", 1.0);

            info!("Client {} disconnected", client_id);
            self.events.push_disconnects(client_id);
            self.user_connections.remove(&client_id);
            self.room_manager.client_disconnect(client_id);
        }
        Ok(())
    }

    /// Receive messages from each connection, and update the events buffer
    pub fn receive(&mut self, world: &mut World) {
        for (client_id, connection) in &mut self.user_connections.iter_mut() {
            let _span = trace_span!("receive", client_id = ?client_id).entered();
            let connection_events = connection.receive(world, &self.time_manager);
            if !connection_events.is_empty() {
                self.events.push_events(*client_id, connection_events);
            }
        }
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let span = trace_span!("send_packets").entered();
        for (client_idx, connection) in &mut self.user_connections.iter_mut() {
            let client_span =
                trace_span!("send_packets_to_client", client_id = ?client_idx).entered();
            for packet_byte in connection
                .base
                .send_packets(&self.time_manager, &self.tick_manager)?
            {
                self.netcode
                    .send(packet_byte.as_slice(), *client_idx, &mut self.io)?;
            }
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self, bevy_tick: BevyTick) -> Result<()> {
        while let Some((mut reader, client_id)) = self.netcode.recv() {
            // TODO: use connection to apply on BOTH message manager and replication manager
            self.user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .recv_packet(&mut reader, bevy_tick)?;
        }
        Ok(())
    }

    pub fn room_mut(&mut self, id: RoomId) -> RoomMut {
        RoomMut {
            id,
            manager: &mut self.room_manager,
        }
    }

    pub fn room(&self, id: RoomId) -> RoomRef {
        RoomRef {
            id,
            manager: &self.room_manager,
        }
    }
}

pub struct ServerContext {
    pub connections: crossbeam_channel::Receiver<ClientId>,
    pub disconnections: crossbeam_channel::Receiver<ClientId>,
}

impl<P: Protocol> ReplicationSend<P> for Server<P> {
    fn new_connected_clients(&self) -> &Vec<ClientId> {
        &self.new_clients
    }

    fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group = replicate.group_id(Some(entity));
        // debug!(?entity, "Spawning entity");
        // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
        self.apply_replication(target).try_for_each(|client_id| {
            trace!(
                ?client_id,
                ?entity,
                "Send entity spawn for tick {:?}",
                self.tick_manager.current_tick()
            );
            let replication_manager = &mut self
                .user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .replication_manager;
            // update the collect changes tick
            replication_manager
                .group_channels
                .entry(group)
                .or_default()
                .update_collect_changes_since_this_tick(system_current_tick);
            replication_manager.prepare_entity_spawn(entity, group);
            // if we need to do prediction/interpolation, send a marker component to indicate that to the client
            if replicate.prediction_target.should_send_to(&client_id) {
                replication_manager.prepare_component_insert(
                    entity,
                    group,
                    P::Components::from(ShouldBePredicted),
                );
            }
            if replicate.interpolation_target.should_send_to(&client_id) {
                replication_manager.prepare_component_insert(
                    entity,
                    group,
                    P::Components::from(ShouldBeInterpolated),
                );
            }
            Ok(())
        })
    }

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let group = replicate.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            trace!(
                ?entity,
                ?client_id,
                "Send entity despawn for tick {:?}",
                self.tick_manager.current_tick()
            );
            let replication_manager = &mut self
                .user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .replication_manager;
            // update the collect changes tick
            replication_manager
                .group_channels
                .entry(group)
                .or_default()
                .update_collect_changes_since_this_tick(system_current_tick);
            replication_manager.prepare_entity_despawn(entity, group);
            Ok(())
        })
    }

    // TODO: perf gain if we batch this? (send vec of components) (same for update/removes)
    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        let group = replicate.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            trace!(
                ?entity,
                component = ?kind,
                tick = ?self.tick_manager.current_tick(),
                "Inserting single component"
            );
            let replication_manager = &mut self
                .user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .replication_manager;
            // update the collect changes tick
            replication_manager
                .group_channels
                .entry(group)
                .or_default()
                .update_collect_changes_since_this_tick(system_current_tick);
            replication_manager.prepare_component_insert(entity, group, component.clone());
            Ok(())
        })
    }

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        debug!(?entity, ?component_kind, "Sending RemoveComponent");
        let group = replicate.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            let replication_manager = &mut self
                .user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .replication_manager;
            replication_manager
                .group_channels
                .entry(group)
                .or_default()
                .update_collect_changes_since_this_tick(system_current_tick);
            replication_manager.prepare_component_remove(entity, group, component_kind.clone());
            Ok(())
        })
    }

    fn prepare_entity_update(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
        target: NetworkTarget,
        component_change_tick: BevyTick,
        system_current_tick: BevyTick,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();

        let group = replicate.group_id(Some(entity));
        self.apply_replication(target).try_for_each(|client_id| {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            let replication_manager = &mut self
                .user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .replication_manager;
            let last_updates_ack_bevy_tick = replication_manager
                .group_channels
                .entry(group)
                .or_default()
                .collect_changes_since_this_tick;
            // send the update for all changes newer than the last ack bevy tick for the group

            if component_change_tick.is_newer_than(last_updates_ack_bevy_tick, system_current_tick)
            {
                trace!(
                    change_tick = ?component_change_tick,
                    last_ack_tick = ?last_updates_ack_bevy_tick,
                    current_tick = ?system_current_tick,
                    "prepare entity update changed check"
                );
                trace!(
                    ?entity,
                    component = ?kind,
                    tick = ?self.tick_manager.current_tick(),
                    "Updating single component"
                );
                replication_manager.prepare_entity_update(entity, group, component.clone());
            }
            Ok(())
        })
    }

    /// Buffer the replication messages
    fn buffer_replication_messages(&mut self) -> Result<()> {
        let _span = trace_span!("buffer_replication_messages").entered();
        self.netcode
            .connected_client_ids()
            .iter()
            .try_for_each(|client_id| {
                self.user_connections
                    .get_mut(client_id)
                    .context("client not found")?
                    .base
                    .buffer_replication_messages(self.tick_manager.current_tick())
            })
    }
}

impl<P: Protocol> TickManaged for Server<P> {
    fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }
}
