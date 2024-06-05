//! Defines the server bevy systems and run conditions
use crate::connection::server::{IoConfig, NetServer, ServerConnection, ServerConnections};
use crate::prelude::{
    is_started, ChannelRegistry, MainSet, MessageRegistry, TickManager, TimeManager,
};
use crate::protocol::component::ComponentRegistry;
use crate::server::clients::ControlledEntities;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::error::ServerError;
use crate::server::events::{ConnectEvent, DisconnectEvent};
use crate::server::io::ServerIoEvent;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use crate::shared::time_manager::is_server_ready_to_send;
use async_channel::TryRecvError;
use bevy::ecs::system::{RunSystemOnce, SystemChangeTick};
use bevy::prelude::*;
use tracing::{debug, error, trace};

/// Plugin handling the server networking systems: sending/receiving packets to clients
#[derive(Default)]
pub(crate) struct ServerNetworkingPlugin;

// TODO: have more parallelism here
// - receive/send packets in parallel
// - update connections in parallel
// - update multiple transports in parallel
// maybe by having each connection or each transport be a separate entity? and then use par_iter?

impl Plugin for ServerNetworkingPlugin {
    fn build(&self, app: &mut App) {
        app
            // REFLECTION
            .register_type::<IoConfig>()
            // STATE
            .init_state::<NetworkingState>()
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                (
                    InternalMainSet::<ServerMarker>::Receive.in_set(MainSet::Receive),
                    InternalMainSet::<ServerMarker>::EmitEvents.in_set(MainSet::EmitEvents),
                )
                    .chain()
                    .run_if(is_started),
            )
            .configure_sets(
                PostUpdate,
                (
                    // we don't send packets every frame, but on a timer instead
                    InternalMainSet::<ServerMarker>::Send
                        .in_set(MainSet::Send)
                        .run_if(is_started.and_then(is_server_ready_to_send)),
                    InternalMainSet::<ServerMarker>::SendPackets
                        .in_set(MainSet::SendPackets)
                        .in_set(InternalMainSet::<ServerMarker>::Send),
                ),
            )
            // SYSTEMS //
            .add_systems(
                PreUpdate,
                receive.in_set(InternalMainSet::<ServerMarker>::Receive),
            )
            .add_systems(
                PostUpdate,
                send.in_set(InternalMainSet::<ServerMarker>::SendPackets),
            );

        // STARTUP
        // create the server connection resources to avoid some systems panicking
        // TODO: remove this when possible?
        app.world.run_system_once(rebuild_server_connections);

        // ON_START
        app.add_systems(OnEnter(NetworkingState::Started), on_start);

        // ON_STOP
        app.add_systems(OnEnter(NetworkingState::Stopped), on_stop);
    }
}

pub(crate) fn receive(world: &mut World) {
    trace!("Receive client packets");
    world.resource_scope(|world: &mut World, mut connection_manager: Mut<ConnectionManager>| {
        world.resource_scope(
            |world: &mut World, mut netservers: Mut<ServerConnections>| {
                    world.resource_scope(
                        |world: &mut World, mut time_manager: Mut<TimeManager>| {
                            world.resource_scope(
                                |world: &mut World, tick_manager: Mut<TickManager>| {
                                            let delta = world.resource::<Time<Virtual>>().delta();
                                            // UPDATE: update server state, send keep-alives, receive packets from io
                                            // update time manager
                                            time_manager.update(delta);
                                            trace!(time = ?time_manager.current_time(), tick = ?tick_manager.tick(), "receive");

                                            // update server net connections
                                            // reborrow trick to enable split borrows
                                            let netservers = &mut *netservers;
                                            for (server_idx, netserver) in netservers.servers.iter_mut().enumerate() {
                                                // TODO: maybe run this before receive, like for clients?
                                                let mut to_disconnect = vec![];
                                                if let Some(io) = netserver.io_mut() {
                                                    if let Some(receiver) = &mut io.context.event_receiver {
                                                        match receiver.try_recv() {
                                                            Ok(event) => {
                                                                match event {
                                                                    // if the io task for any connection failed, disconnect the client in netcode
                                                                    ServerIoEvent::ClientDisconnected(client_id) => {
                                                                        to_disconnect.push(client_id);
                                                                    }
                                                                    ServerIoEvent::ServerDisconnected(e) => {
                                                                        error!("Disconnect server because of io error: {:?}", e);
                                                                        world.resource_mut::<NextState<NetworkingState>>().set(NetworkingState::Stopped);
                                                                    }
                                                                    _ => {}
                                                                }
                                                            }
                                                            Err(TryRecvError::Empty) => {}
                                                            Err(TryRecvError::Closed) => {}
                                                        }
                                                    }
                                                }


                                                let _ = netserver
                                                    .try_update(delta.as_secs_f64())
                                                    .map_err(|e| error!("Error updating netcode server: {:?}", e));
                                                for client_id in netserver.new_connections().iter().copied() {
                                                    netservers.client_server_map.insert(client_id, server_idx);
                                                    // spawn an entity for the client
                                                    let client_entity = world.spawn((ControlledEntities::default(), Name::new("Client"))).id();
                                                    connection_manager.add(client_id, client_entity);
                                                }
                                                // handle disconnections

                                                // disconnections because the io task was closed
                                                if !to_disconnect.is_empty() {
                                                    to_disconnect.into_iter().for_each(|addr| {
                                                        #[allow(irrefutable_let_patterns)]
                                                        if let ServerConnection::Netcode(server) = netserver {
                                                            error!("Disconnecting client {addr:?} because of io error");
                                                            let _ = server.disconnect_by_addr(addr);
                                                        }
                                                    })
                                                }
                                                // disconnects because we received a disconnect message
                                                for client_id in netserver.new_disconnections().iter().copied() {
                                                    if netservers.client_server_map.remove(&client_id).is_some() {
                                                        connection_manager.remove(client_id);
                                                        // NOTE: we don't despawn the entity right away to let the user react to
                                                        // the disconnect event
                                                        // TODO: use observers/component_hooks to react automatically on the client despawn?
                                                        // world.despawn(client_entity);
                                                    } else {
                                                        error!("Client disconnected but could not map client_id to the corresponding netserver");
                                                    }
                                                };
                                            }

                                            // update connections
                                            connection_manager
                                                .update(world.change_tick(), time_manager.as_ref(), tick_manager.as_ref());

                                            // RECV_PACKETS: buffer packets into message managers
                                            // enable split borrows on connection manager
                                            let connection_manager = &mut *connection_manager;
                                            for (server_idx, netserver) in netservers.servers.iter_mut().enumerate() {
                                                while let Some((payload, client_id)) = netserver.recv() {
                                                    // Note: the client_id might not be present in the connection_manager if we receive
                                                    // packets from a client
                                                    // TODO: use connection to apply on BOTH message manager and replication manager
                                                    if let Some(connection) = connection_manager
                                                        .connections.get_mut(&client_id) {
                                                        let component_registry = world.resource::<ComponentRegistry>();
                                                        connection.recv_packet(payload, tick_manager.as_ref(), component_registry, &mut connection_manager.delta_manager).expect("could not receive packet");
                                                    } else {
                                                        // it's still possible to receive some packets from a client that just disconnected.
                                                        // (multiple packets arrived at the same time from that client)
                                                        if netserver.new_disconnections().contains(&client_id) {
                                                            trace!("received packet from client that just got disconnected. Ignoring.");
                                                            // we ignore packets from disconnected clients
                                                            // this is not an error
                                                            continue;
                                                        } else {
                                                            error!("Received packet from unknown client: {}", client_id);
                                                        }
                                                    }
                                                }
                                            }

                                            // RECEIVE: read messages and parse them into events
                                            connection_manager
                                                .receive(world, time_manager.as_ref(), tick_manager.as_ref())
                                                .unwrap_or_else(|e| {
                                                    error!("Error during receive: {}", e);
                                                });

                                            // EVENTS: Write the received events into bevy events
                                            if !connection_manager.events.is_empty() {
                                                // Connection / Disconnection events
                                                if connection_manager.events.has_connections() {
                                                    let mut connect_event_writer =
                                                        world.get_resource_mut::<Events<ConnectEvent>>().unwrap();
                                                    for connect_event in connection_manager.events.iter_connections() {
                                                        debug!("Client connected event: {}", connect_event.client_id);
                                                        connect_event_writer.send(connect_event);
                                                    }
                                                }

                                                if connection_manager.events.has_disconnections() {
                                                    let mut connect_event_writer =
                                                        world.get_resource_mut::<Events<DisconnectEvent>>().unwrap();
                                                    for disconnect_event in connection_manager.events.iter_disconnections() {
                                                        debug!("Client disconnected event: {}", disconnect_event.client_id);
                                                        connect_event_writer.send(disconnect_event);
                                                    }
                                                }
                                            }
                                });
                        });
                });
    });
}

// or do additional send stuff here
pub(crate) fn send(
    change_tick: SystemChangeTick,
    mut netservers: ResMut<ServerConnections>,
    mut connection_manager: ResMut<ConnectionManager>,
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
) {
    trace!("Send packets to clients");
    // finalize any packets that are needed for replication
    connection_manager
        .buffer_replication_messages(tick_manager.tick(), change_tick.this_run())
        .unwrap_or_else(|e| {
            error!("Error preparing replicate send: {}", e);
        });

    // SEND_PACKETS: send buffered packets to io
    let span = info_span!("send_packets").entered();
    connection_manager
        .connections
        .iter_mut()
        .try_for_each(|(client_id, connection)| {
            let client_span =
                info_span!("send_packets_to_client", client_id = ?client_id).entered();
            let netserver_idx = *netservers
                .client_server_map
                .get(client_id)
                .ok_or(ServerError::ServerConnectionNotFound)?;
            let netserver = netservers
                .servers
                .get_mut(netserver_idx)
                .ok_or(ServerError::ServerConnectionNotFound)?;
            for packet_byte in connection.send_packets(&time_manager, &tick_manager)? {
                netserver.send(packet_byte.as_slice(), *client_id)?;
            }
            Ok(())
        })
        .unwrap_or_else(|e: ServerError| {
            error!("Error sending packets: {}", e);
        });

    // clear the list of newly connected clients
    // (cannot just use the ConnectionEvent because it is cleared after each frame)
    connection_manager.new_clients.clear();
}

/// Bevy [`State`] representing the networking state of the server.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkingState {
    /// The server is not listening. The server plugin is disabled.
    #[default]
    Stopped,
    // NOTE: there is no need for a `Starting` state because currently the server
    // `start` method is synchronous. Once it returns we know the server is started and ready.
    /// The server is ready to accept incoming connections.
    Started,
}

/// This runs only when we restart the server.
///
/// We rebuild the [`ServerConnections`] by using the latest [`ServerConfig`].
/// This has several benefits:
/// - the server connection's internal time is up-to-date (otherwise it might not be, since we don't run any server systems while the server is stopped)
/// - we can take into account any changes to the server config
fn rebuild_server_connections(world: &mut World) {
    let server_config = world.resource::<ServerConfig>().clone();

    // insert a new connection manager (to reset message numbers, ping manager, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<MessageRegistry>().clone(),
        world.resource::<ChannelRegistry>().clone(),
        server_config.replication,
        server_config.packet,
        server_config.ping,
    );
    world.insert_resource(connection_manager);

    // rebuild the server connections and insert them
    let server_connections = ServerConnections::new(server_config.net);
    world.insert_resource(server_connections);
}

/// System that runs when we enter the Started state
/// - rebuild the server connections resource from the latest `ServerConfig`
/// - rebuild the server connection manager
/// - start listening on the server connections
fn on_start(world: &mut World) {
    if world.resource::<ServerConnections>().is_listening() {
        error!("The server is already started. The server can only be started when it is stopped.");
        return;
    }
    rebuild_server_connections(world);
    let _ = world
        .resource_mut::<ServerConnections>()
        .start()
        .inspect_err(|e| error!("Error starting server connections: {:?}", e));
}

/// System that runs when we enter the Stopped state
fn on_stop(mut server_connections: ResMut<ServerConnections>) {
    let _ = server_connections
        .stop()
        .inspect_err(|e| error!("Error stopping server connections: {:?}", e));
}

pub trait ServerCommands {
    fn start_server(&mut self);

    fn stop_server(&mut self);
}

impl ServerCommands for Commands<'_, '_> {
    fn start_server(&mut self) {
        self.insert_resource(NextState::<NetworkingState>(Some(NetworkingState::Started)));
    }

    fn stop_server(&mut self) {
        self.insert_resource(NextState::<NetworkingState>(Some(NetworkingState::Stopped)));
    }
}
