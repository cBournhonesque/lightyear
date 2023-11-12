use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use bevy::prelude::{Resource, Time, World};
use tracing::{debug, debug_span, info, trace_span};

use super::connection::Connection;
use crate::netcode::{generate_key, ClientId, ConnectToken};
use crate::replication::prediction::ShouldBePredicted;
use crate::replication::{Replicate, ReplicationSend, ReplicationTarget};
use crate::tick::{Tick, TickManaged};
use crate::transport::{PacketSender, Transport};
use crate::{Channel, ChannelKind, Entity, Io, Message, Protocol, SyncMessage, TickManager};
use crate::{TimeManager, WriteBuffer};

use super::config::ServerConfig;
use super::events::ServerEvents;
use super::io::NetcodeServerContext;

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
    // Protocol
    pub protocol: P,
    // Events
    events: ServerEvents<P>,
    // Time
    time_manager: TimeManager,
    tick_manager: TickManager,
}

impl<P: Protocol> Server<P> {
    pub fn new(config: ServerConfig, protocol: P) -> Self {
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
        let io = Io::from_config(&config.io).expect("Could not create io");
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
            protocol,
            events: ServerEvents::new(),
            time_manager: TimeManager::new(),
            tick_manager: TickManager::from_config(config.shared.tick),
        }
    }

    /// Generate a connect token for a client with id `client_id`
    pub fn token(&mut self, client_id: ClientId) -> ConnectToken {
        self.netcode.token(client_id, &self.io).generate().unwrap()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.io.local_addr()
    }

    // pub fn client_id(&self, addr: SocketAddr) -> Option<ClientId> {
    //     self.netcode.client_ids()
    // }

    pub fn client_ids(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.netcode.client_ids()
    }

    // TICK

    pub fn tick(&self) -> Tick {
        self.tick_manager.current_tick()
    }
    pub(crate) fn increment_tick(&mut self) {
        self.tick_manager.increment_tick()
    }

    // REPLICATION

    fn apply_replication<F: Fn(ClientId, &Replicate, &mut Connection<P>) -> Result<()>>(
        &mut self,
        replicate: &Replicate,
        f: F,
    ) -> Result<()> {
        match replicate.target {
            ReplicationTarget::All => {
                for client_id in self.netcode.connected_client_ids() {
                    let connection = self
                        .user_connections
                        .get_mut(&client_id)
                        .expect("client not found");
                    f(client_id, replicate, connection)?;
                }
            }
            ReplicationTarget::AllExcept(client_id) => {
                for client_id in self
                    .netcode
                    .connected_client_ids()
                    .filter(|id| *id != client_id)
                {
                    let connection = self
                        .user_connections
                        .get_mut(&client_id)
                        .expect("client not found");
                    f(client_id, replicate, connection)?;
                }
            }
            ReplicationTarget::Only(client_id) => {
                let connection = self
                    .user_connections
                    .get_mut(&client_id)
                    .expect("client not found");
                f(client_id, replicate, connection)?;
            }
        }
        Ok(())
    }

    // MESSAGES

    /// Queues up a message to be sent to all clients
    pub fn broadcast_send<C: Channel, M: Message>(&mut self, message: M) -> Result<()>
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
        for client_id in self.netcode.connected_client_ids() {
            self.user_connections
                .get_mut(&client_id)
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
    // TODO: change the argument to delta?
    pub fn update(&mut self, delta: Duration) -> Result<()> {
        // update time manager
        self.time_manager.update(delta);
        // self.tick_manager.update(delta);

        // update netcode server
        self.netcode
            .try_update(delta.as_secs_f64(), &mut self.io)
            .context("Error updating netcode server")?;

        // update connections
        for (_, connection) in &mut self.user_connections {
            connection.update(delta, &self.time_manager, &self.tick_manager);
        }

        // handle connections
        for client_id in self.context.connections.try_iter() {
            #[cfg(feature = "metrics")]
            metrics::increment_gauge!("connected_clients", 1.0);

            let client_addr = self.netcode.client_addr(client_id).unwrap();
            debug!("New connection from {} (id: {})", client_addr, client_id);
            let mut connection =
                Connection::new(self.protocol.channel_registry(), &self.config.ping);
            connection.base.events.push_connection();
            self.user_connections.insert(client_id, connection);
        }

        // handle disconnections
        for client_id in self.context.disconnections.try_iter() {
            #[cfg(feature = "metrics")]
            metrics::decrement_gauge!("connected_clients", 1.0);

            debug!("Client {} got disconnected", client_id);
            self.events.push_disconnects(client_id);
            self.user_connections.remove(&client_id);
        }
        Ok(())
    }

    pub fn receive(&mut self, world: &mut World) -> ServerEvents<P> {
        for (client_id, connection) in &mut self.user_connections.iter_mut() {
            trace_span!("receive", client_id = ?client_id).entered();
            let mut connection_events = connection.base.receive(world, &self.time_manager);

            // handle sync events
            for sync in connection_events.into_iter_syncs() {
                match sync {
                    SyncMessage::Ping(ping) => {
                        connection.buffer_pong(&self.time_manager, ping).unwrap();
                    }
                    SyncMessage::Pong(_) => {}
                    SyncMessage::TimeSyncPing(ping) => {
                        connection
                            .buffer_sync_pong(&self.time_manager, &self.tick_manager, ping)
                            .unwrap();
                    }
                    SyncMessage::TimeSyncPong(_) => {
                        panic!("only the server sends time-sync-pong messages")
                    }
                }
            }
            // handle pings
            for ping in connection_events.into_iter_pings() {
                connection.buffer_pong(&self.time_manager, ping).unwrap();
            }
            // handle pongs
            for pong in connection_events.into_iter_pongs() {
                // update rtt/jitter estimation + update ping store
                info!("Process pong {:?}", pong);
                connection
                    .ping_manager
                    .process_pong(pong, &self.time_manager);
            }
            if !connection_events.is_empty() {
                self.events.push_events(*client_id, connection_events);
            }
        }

        // return all received messages and reset the buffer
        std::mem::replace(&mut self.events, ServerEvents::new())
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let span = trace_span!("send_packets").entered();
        for (client_idx, connection) in &mut self.user_connections.iter_mut() {
            let client_span =
                trace_span!("send_packets_to_client", client_id = ?client_idx).entered();
            for mut packet_byte in connection.base.send_packets(&self.tick_manager)? {
                self.netcode
                    .send(packet_byte.as_slice(), *client_idx, &mut self.io)?;
            }
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> Result<()> {
        while let Some((mut reader, client_id)) = self.netcode.recv() {
            // TODO: use connection to apply on BOTH message manager and replication manager
            self.user_connections
                .get_mut(&client_id)
                .context("client not found")?
                .base
                .recv_packet(&mut reader)?;
        }
        Ok(())
    }
}

pub struct ServerContext {
    pub connections: crossbeam_channel::Receiver<ClientId>,
    pub disconnections: crossbeam_channel::Receiver<ClientId>,
}

impl<P: Protocol> ReplicationSend<P> for Server<P> {
    fn entity_spawn(
        &mut self,
        entity: Entity,
        mut components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()> {
        // debug!(?entity, "Spawning entity");
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            // if we need to do prediction, send a marker component to indicate that to the client
            let mut components = components.clone();
            if replicate.should_do_prediction {
                components.push(P::Components::from(ShouldBePredicted));
            }
            connection
                .base
                .buffer_spawn_entity(entity, components, replicate.channel)
            // if replicate.should_do_prediction {
            //     connection.base.buffer_component_insert(entity, , replicate.channel)
            //
            // }
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn entity_despawn(&mut self, entity: Entity, replicate: &Replicate) -> Result<()> {
        debug!(?entity, "Sending EntityDespawn");
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection
                .base
                .buffer_despawn_entity(entity, replicate.channel)
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        debug!(
            ?entity,
            component = ?kind,
            "Inserting single component"
        );
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.base.buffer_update_entity_single_component(
                entity,
                component.clone(),
                replicate.channel,
            )
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate,
    ) -> Result<()> {
        debug!(?entity, ?component_kind, "Sending RemoveComponent");
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.base.buffer_component_remove(
                entity,
                component_kind.clone(),
                replicate.channel,
            )
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn entity_update_single_component(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()> {
        let kind: P::ComponentKinds = (&component).into();
        debug!(
            ?entity,
            component = ?kind,
            "Updating single component"
        );
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.base.buffer_update_entity_single_component(
                entity,
                component.clone(),
                replicate.channel,
            )
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn entity_update(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()> {
        debug!("Updating components for entity {:?}", entity);
        let mut buffer_message = |client_id: ClientId,
                                  replicate: &Replicate,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection
                .base
                .buffer_update_entity(entity, components.clone(), replicate.channel)
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn prepare_replicate_send(&mut self) {
        // debug!("Finalizing replication messages on server");
        let span = trace_span!("prepare_replicate_send").entered();
        for client_id in self.netcode.connected_client_ids() {
            let connection = self
                .user_connections
                .get_mut(&client_id)
                .expect("client not found");
            connection.base.prepare_replication_send();
        }
    }
}

impl<P: Protocol> TickManaged for Server<P> {
    fn increment_tick(&mut self) {
        self.tick_manager.increment_tick();
    }
}
