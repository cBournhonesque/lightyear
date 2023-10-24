use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use bevy::prelude::{Resource, World};
use tracing::{debug, trace_span};

use lightyear_shared::netcode::{generate_key, ClientId, ConnectToken};
use lightyear_shared::replication::{Replicate, ReplicationSend, ReplicationTarget};
use lightyear_shared::transport::{PacketSender, Transport};
use lightyear_shared::{Channel, ChannelKind, Entity, Io, Protocol};
use lightyear_shared::{Connection, WriteBuffer};

use crate::events::ServerEvents;
use crate::io::NetcodeServerContext;
use crate::ServerConfig;

#[derive(Resource)]
pub struct Server<P: Protocol> {
    // Config

    // Io
    io: Io,
    // Netcode
    netcode: lightyear_shared::netcode::Server<NetcodeServerContext>,
    context: ServerContext,
    // Clients
    user_connections: HashMap<ClientId, Connection<P>>,
    // Protocol
    protocol: P,
    // Events
    events: ServerEvents<P>,
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
        let mut cfg = lightyear_shared::netcode::ServerConfig::with_context(server_context)
            .on_connect(|id, ctx| {
                ctx.connections.send(id).unwrap();
            })
            .on_disconnect(|id, ctx| {
                ctx.disconnections.send(id).unwrap();
            });
        cfg = cfg.keep_alive_send_rate(config.netcode.keep_alive_send_rate);
        cfg = cfg.num_disconnect_packets(config.netcode.num_disconnect_packets);

        let netcode = lightyear_shared::netcode::Server::with_config(
            config.netcode.protocol_id,
            private_key,
            cfg,
        )
        .expect("Could not create server netcode");
        let io = Io::from_config(config.io).expect("Could not create io");
        let context = ServerContext {
            connections: connections_rx,
            disconnections: disconnections_rx,
        };
        Self {
            io,
            netcode,
            context,
            user_connections: HashMap::new(),
            protocol,
            events: ServerEvents::new(),
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

    // REPLICATION

    fn apply_replication<F: Fn(ClientId, ChannelKind, &mut Connection<P>) -> Result<()>>(
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
                    f(client_id, replicate.channel, connection)?;
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
                    f(client_id, replicate.channel, connection)?;
                }
            }
            ReplicationTarget::Only(client_id) => {
                let connection = self
                    .user_connections
                    .get_mut(&client_id)
                    .expect("client not found");
                f(client_id, replicate.channel, connection)?;
            }
        }
        Ok(())
    }

    // MESSAGES

    /// Queues up a message to be sent to a client
    pub fn buffer_send<C: Channel>(
        &mut self,
        client_id: ClientId,
        message: P::Message,
    ) -> Result<()> {
        let channel = ChannelKind::of::<C>();
        self.user_connections
            .get_mut(&client_id)
            .context("client not found")?
            .buffer_message(message, channel)
    }

    /// Update the server's internal state, queues up in a buffer any packets received from clients
    /// Sends keep-alive packets + any non-payload packet needed for netcode
    pub fn update(&mut self, time: f64) -> Result<()> {
        // update netcode server
        self.netcode
            .try_update(time, &mut self.io)
            .context("Error updating netcode server")?;

        // handle connections
        for client_id in self.context.connections.try_iter() {
            let client_addr = self.netcode.client_addr(client_id).unwrap();
            let connection = Connection::new(self.protocol.channel_registry());
            debug!("New connection from {} (id: {})", client_addr, client_id);
            self.events.push_connections(client_id);
            self.user_connections.insert(client_id, connection);
        }

        // handle disconnections
        for client_id in self.context.disconnections.try_iter() {
            debug!("Client {} got disconnected", client_id);
            self.events.push_disconnects(client_id);
            self.user_connections.remove(&client_id);
        }
        Ok(())
    }

    pub fn receive(&mut self, world: &mut World) -> ServerEvents<P> {
        for (client_id, connection) in &mut self.user_connections.iter_mut() {
            let connection_events = connection.receive(world);
            if !connection_events.is_empty() {
                self.events.push_events(*client_id, connection_events);
            }
        }
        // return all received messages and reset the buffer
        std::mem::replace(&mut self.events, ServerEvents::new())
    }

    // /// Receive messages from the server
    // pub fn read_messages(
    //     &mut self,
    //     client_id: ClientId,
    // ) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
    //     if let Some(connection) = self.user_connections.get_mut(&client_id) {
    //         connection.message_manager.read_messages()
    //     } else {
    //         HashMap::new()
    //     }
    // }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> Result<()> {
        let span = trace_span!("send_packets").entered();
        for (client_idx, connection) in &mut self.user_connections.iter_mut() {
            let client_span =
                trace_span!("send_packets_to_client", client_id = ?client_idx).entered();
            for mut packet_byte in connection.send_packets()? {
                self.netcode
                    .send(packet_byte.finish_write(), *client_idx, &mut self.io)?;
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
                .message_manager
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
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()> {
        // debug!(?entity, "Spawning entity");
        let mut buffer_message = |client_id: ClientId,
                                  channel: ChannelKind,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.buffer_spawn_entity(entity, components.clone(), channel)
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn entity_despawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()> {
        todo!()
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
                                  channel: ChannelKind,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.buffer_update_entity_single_component(entity, component.clone(), channel)
        };
        self.apply_replication(replicate, buffer_message)
    }

    fn component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate,
    ) -> Result<()> {
        todo!()
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
                                  channel: ChannelKind,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.buffer_update_entity_single_component(entity, component.clone(), channel)
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
                                  channel: ChannelKind,
                                  connection: &mut Connection<P>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            connection.buffer_update_entity(entity, components.clone(), channel)
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
            connection.prepare_replication_send();
        }
    }
}
