use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::Context;
use bevy_ecs::entity::Entity;
use log::debug;

use lightyear_shared::netcode::{generate_key, ClientId, ClientIndex, ConnectToken, ServerConfig};
use lightyear_shared::replication::{Replicate, ReplicationMessage, ReplicationTarget};
use lightyear_shared::transport::{PacketSender, Transport};
use lightyear_shared::WriteBuffer;
use lightyear_shared::{ChannelKind, ChannelRegistry, Connection, Io, MessageContainer, Protocol};

use crate::io::NetcodeServerContext;

pub struct Server<P: Protocol> {
    // Config

    // Io
    io: Io,
    // Netcode
    netcode: lightyear_shared::netcode::Server<NetcodeServerContext>,
    context: ServerContext,
    // Clients
    user_connections: HashMap<ClientIndex, Connection<P>>,
    // Protocol
    channel_registry: &'static ChannelRegistry,
}

impl<P: Protocol> Server<P> {
    pub fn new(io: Io, protocol_id: u64, channel_registry: &'static ChannelRegistry) -> Self {
        // create netcode server
        let private_key = generate_key();
        let (connections_tx, connections_rx) = crossbeam_channel::unbounded();
        let (disconnections_tx, disconnections_rx) = crossbeam_channel::unbounded();
        let server_context = NetcodeServerContext {
            connections: connections_tx,
            disconnections: disconnections_tx,
        };
        let cfg = ServerConfig::with_context(server_context)
            .on_connect(|idx, ctx| {
                ctx.connections.send(idx).unwrap();
            })
            .on_disconnect(|idx, ctx| {
                ctx.disconnections.send(idx).unwrap();
            });
        let netcode =
            lightyear_shared::netcode::Server::with_config(protocol_id, private_key, cfg).unwrap();
        let context = ServerContext {
            connections: connections_rx,
            disconnections: disconnections_rx,
        };
        Self {
            io,
            netcode,
            context,
            user_connections: HashMap::new(),
            channel_registry,
        }
    }

    /// Generate a connect token for a client with id `client_id`
    pub fn token(&mut self, client_id: ClientId) -> ConnectToken {
        self.netcode
            .token(client_id, &mut self.io)
            .generate()
            .unwrap()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.io.local_addr()
    }

    pub fn client_idx(&self, addr: SocketAddr) -> Option<ClientIndex> {
        self.netcode.client_index(&addr)
    }
    pub fn client_idxs(&self) -> impl Iterator<Item = ClientIndex> + '_ {
        self.netcode.client_idxs()
    }

    // REPLICATION

    pub fn entity_spawn(&mut self, entity: Entity, replicate: &Replicate) {
        let channel_kind = replicate.channel.unwrap();
        // TODO: use a pre-existing reliable channel for entity actions
        // let channel_kind = replicate.channel.unwrap_or(default_reliable_channel);

        let buffer_message = |client_idx: ClientIndex| {
            let message = MessageContainer::new(ReplicationMessage::<P>::SpawnEntity(entity));
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            self.buffer_send(client_idx, message, channel_kind);
        };

        match replicate.target {
            ReplicationTarget::All => {
                for client_idx in self.client_idxs() {
                    buffer_message(client_idx);
                }
            }
            ReplicationTarget::AllExcept(client_id) => {
                // TODO: convert to client_idx
                self.client_idxs()
                    .filter(|idx| idx != client_id)
                    .for_each(buffer_message);
            }
            ReplicationTarget::Only(client_id) => {
                // TODO: convert to client_idx
                buffer_message(client_idx);
            }
        }
    }

    // MESSAGES

    /// Queues up a message to be sent to a client
    pub fn buffer_send(
        &mut self,
        client_id: ClientIndex,
        message: MessageContainer<P::Message>,
        channel_kind: ChannelKind,
    ) -> anyhow::Result<()> {
        self.user_connections
            .get_mut(&client_id)
            .context("client not found")?
            .buffer_send(message, channel_kind)
    }

    /// Update the server's internal state, queues up in a buffer any packets received from clients
    /// Sends keep-alive packets + any non-payload packet needed for netcode
    pub fn update(&mut self, time: f64) -> anyhow::Result<()> {
        // update netcode server
        self.netcode
            .try_update(time, &mut self.io)
            .context("Error updating netcode server")?;

        // handle connections
        for client_idx in self.context.connections.try_iter() {
            let client_addr = self.netcode.client_addr(client_idx).unwrap();
            let connection = Connection::new(self.channel_registry);
            debug!(
                "New connection from {} (index: {})",
                client_addr, client_idx
            );
            self.user_connections.insert(client_idx, connection);
        }

        // handle disconnections
        for client_id in self.context.disconnections.try_iter() {
            debug!("Client {} got disconnected", client_id);
            self.user_connections.remove(&client_id);
        }
        Ok(())
    }

    /// Receive messages from the server
    /// TODO: maybe use events?
    pub fn read_messages(
        &mut self,
        client_id: ClientIndex,
    ) -> HashMap<ChannelKind, Vec<MessageContainer<P::Message>>> {
        if let Some(connection) = self.user_connections.get_mut(&client_id) {
            connection.read_messages()
        } else {
            HashMap::new()
        }
    }

    /// Send packets that are ready from the message manager through the transport layer
    pub fn send_packets(&mut self) -> anyhow::Result<()> {
        for (client_idx, connection) in &mut self.user_connections.iter_mut() {
            for mut packet_byte in connection.send_packets()? {
                self.netcode
                    .send(packet_byte.finish_write(), *client_idx, &mut self.io)?;
            }
        }
        Ok(())
    }

    /// Receive packets from the transport layer and buffer them with the message manager
    pub fn recv_packets(&mut self) -> anyhow::Result<()> {
        loop {
            match self.netcode.recv() {
                Some((mut reader, client_id)) => {
                    self.user_connections
                        .get_mut(&client_id)
                        .context("client not found")?
                        .recv_packet(&mut reader)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}

pub struct ServerContext {
    pub connections: crossbeam_channel::Receiver<ClientIndex>,
    pub disconnections: crossbeam_channel::Receiver<ClientIndex>,
}
