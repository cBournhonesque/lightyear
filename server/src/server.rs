use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use bevy_ecs::prelude::{Resource, World};
use tracing::debug;

use lightyear_shared::netcode::{generate_key, ClientId, ConnectToken};
use lightyear_shared::replication::{Replicate, ReplicationTarget};
use lightyear_shared::transport::{PacketSender, Transport};
use lightyear_shared::{Channel, Entity, Io, Protocol};
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

    // TODO: MAYBE THE EXTERNAL API SHOULD USE <C> API FOR CLARITY
    //  BUT INTERNALLY WE SHOULD PASS CHANNEL_KINDS AROUND? BECAUSE IT IS EASIER TO USE WITH CHANNEL REGISTRY?
    //  HERE HOW DO WE SPECIFY TO USE THE DEFAULT CHANNEL IF NOT PROVIDED?
    pub fn entity_spawn<C: Channel>(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate<C>,
    ) -> Result<()> {
        let mut buffer_message = |client_id: ClientId,
                                  user_connections: &mut HashMap<ClientId, Connection<P>>|
         -> Result<()> {
            // TODO: should we have additional state tracking so that we know we are in the process of sending this entity to clients?
            user_connections
                .get_mut(&client_id)
                .expect("client not found")
                .buffer_spawn_entity::<C>(entity, components.clone())
        };

        match replicate.target {
            ReplicationTarget::All => {
                let client_ids: Vec<ClientId> = self.client_ids().collect();
                for client_id in client_ids {
                    buffer_message(client_id, &mut self.user_connections)?;
                }
            }
            ReplicationTarget::AllExcept(client_id) => {
                let client_ids: Vec<ClientId> =
                    self.client_ids().filter(|id| *id != client_id).collect();
                for client_id in client_ids {
                    buffer_message(client_id, &mut self.user_connections)?;
                }
            }
            ReplicationTarget::Only(client_id) => {
                buffer_message(client_id, &mut self.user_connections)?;
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
        self.user_connections
            .get_mut(&client_id)
            .context("client not found")?
            .buffer_message::<C>(message)
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
            self.user_connections.insert(client_id, connection);
        }

        // handle disconnections
        for client_id in self.context.disconnections.try_iter() {
            debug!("Client {} got disconnected", client_id);
            self.user_connections.remove(&client_id);
        }
        Ok(())
    }

    pub fn receive(&mut self, world: &mut World) -> ServerEvents<P> {
        let mut events = ServerEvents::new();
        for (client_id, connection) in &mut self.user_connections.iter_mut() {
            let connection_events = connection.receive(world);
            if !connection_events.is_empty() {
                events.push_events(*client_id, connection_events);
            }
        }
        events
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
        for (client_idx, connection) in &mut self.user_connections.iter_mut() {
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
