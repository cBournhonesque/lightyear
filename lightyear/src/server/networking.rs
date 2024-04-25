//! Defines the server bevy systems and run conditions
use anyhow::{anyhow, Context};
use bevy::ecs::system::{RunSystemOnce, SystemChangeTick, SystemParam};
use bevy::prelude::*;
use tracing::{debug, error, trace, trace_span};

use crate::_reexport::{ComponentProtocol, ServerMarker};
use crate::client::config::ClientConfig;
use crate::client::networking::is_disconnected;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::server::{NetConfig, NetServer, ServerConnection, ServerConnections};
use crate::prelude::{MainSet, Mode, TickManager, TimeManager};
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::server::room::RoomManager;
use crate::shared::events::connection::{IterEntityDespawnEvent, IterEntitySpawnEvent};
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::InternalMainSet;
use crate::shared::time_manager::is_server_ready_to_send;

/// Plugin handling the server networking systems: sending/receiving packets to clients
pub(crate) struct ServerNetworkingPlugin<P: Protocol> {
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for ServerNetworkingPlugin<P> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}
impl<P: Protocol> ServerNetworkingPlugin<P> {
    pub(crate) fn new(config: Vec<NetConfig>) -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

// TODO: have more parallelism here
// - receive/send packets in parallel
// - update connections in parallel
// - update multiple transports in parallel
// maybe by having each connection or each transport be a separate entity? and then use par_iter?

impl<P: Protocol> Plugin for ServerNetworkingPlugin<P> {
    fn build(&self, app: &mut App) {
        app
            // STATE
            .init_state::<NetworkingState>()
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                InternalMainSet::<ServerMarker>::Receive.run_if(is_started),
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
        app.world.run_system_once(rebuild_server_connections::<P>);

        // ON_START
        app.add_systems(OnEnter(NetworkingState::Started), on_start::<P>);

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
                                    world.resource_scope(
                                        |world: &mut World, mut room_manager: Mut<RoomManager>| {
                                            let delta = world.resource::<Time<Virtual>>().delta();
                                            // UPDATE: update server state, send keep-alives, receive packets from io
                                            // update time manager
                                            time_manager.update(delta);
                                            trace!(time = ?time_manager.current_time(), tick = ?tick_manager.tick(), "receive");

                                            // update server net connections
                                            // reborrow trick to enable split borrows
                                            let netservers = &mut *netservers;
                                            for (server_idx, netserver) in netservers.servers.iter_mut().enumerate() {
                                                let _ = netserver
                                                    .try_update(delta.as_secs_f64())
                                                    .map_err(|e| error!("Error updating netcode server: {:?}", e));
                                                for client_id in netserver.new_connections().iter().copied() {
                                                    netservers.client_server_map.insert(client_id, server_idx);
                                                    connection_manager.add(client_id);
                                                }
                                                // handle disconnections
                                                for client_id in netserver.new_disconnections().iter().copied() {
                                                    if netservers.client_server_map.remove(&client_id).is_some() {
                                                        connection_manager.remove(client_id);
                                                        room_manager.client_disconnect(client_id);
                                                    } else {
                                                        error!("Client disconnected but could not map client_id to the corresponding netserver");
                                                    }
                                                };
                                            }

                                            // update connections
                                            connection_manager
                                                .update(time_manager.as_ref(), tick_manager.as_ref());

                                            // RECV_PACKETS: buffer packets into message managers
                                            for (server_idx, netserver) in netservers.servers.iter_mut().enumerate() {
                                                while let Some((packet, client_id)) = netserver.recv() {
                                                    // Note: the client_id might not be present in the connection_manager if we receive
                                                    // packets from a client
                                                    // TODO: use connection to apply on BOTH message manager and replication manager
                                                    if let Ok(connection) = connection_manager
                                                        .connection_mut(client_id) {
                                                        connection.recv_packet(packet, tick_manager.as_ref()).expect("could not receive packet");
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

                                            // // EVENTS: Write the received events into bevy events
                                            // if !connection_manager.events.is_empty() {
                                            //     // TODO: write these as systems? might be easier to also add the events to the app
                                            //     //  it might just be less efficient? + maybe tricky to
                                            //     // Input events
                                            //     // Update the input buffers with any InputMessage received:
                                            //
                                            //     // ADD A FUNCTION THAT ITERATES THROUGH EACH CONNECTION AND RETURNS InputEvent for THE CURRENT TICK
                                            //
                                            //     // Connection / Disconnection events
                                            //     if connection_manager.events.has_connections() {
                                            //         let mut connect_event_writer =
                                            //             world.get_resource_mut::<Events<ConnectEvent>>().unwrap();
                                            //         for client_id in connection_manager.events.iter_connections() {
                                            //             debug!("Client connected event: {}", client_id);
                                            //             connect_event_writer.send(ConnectEvent::new(client_id));
                                            //         }
                                            //     }
                                            //
                                            //     if connection_manager.events.has_disconnections() {
                                            //         let mut connect_event_writer =
                                            //             world.get_resource_mut::<Events<DisconnectEvent>>().unwrap();
                                            //         for client_id in connection_manager.events.iter_disconnections() {
                                            //             debug!("Client disconnected event: {}", client_id);
                                            //             connect_event_writer.send(DisconnectEvent::new(client_id));
                                            //         }
                                            //     }
                                            //
                                            //     // Message Events
                                            //     P::Message::push_message_events(world, &mut connection_manager.events);
                                            //
                                            //     // EntitySpawn Events
                                            //     if connection_manager.events.has_entity_spawn() {
                                            //         let mut entity_spawn_event_writer = world
                                            //             .get_resource_mut::<Events<EntitySpawnEvent>>()
                                            //             .unwrap();
                                            //         for (entity, client_id) in connection_manager.events.into_iter_entity_spawn() {
                                            //             entity_spawn_event_writer.send(EntitySpawnEvent::new(entity, client_id));
                                            //         }
                                            //     }
                                            //     // EntityDespawn Events
                                            //     if connection_manager.events.has_entity_despawn() {
                                            //         let mut entity_despawn_event_writer = world
                                            //             .get_resource_mut::<Events<EntityDespawnEvent>>()
                                            //             .unwrap();
                                            //         for (entity, client_id) in connection_manager.events.into_iter_entity_spawn() {
                                            //             entity_despawn_event_writer.send(EntityDespawnEvent::new(entity, client_id));
                                            //         }
                                            //     }
                                            //
                                            //     // Update component events (updates, inserts, removes)
                                            //     P::Components::push_component_events(world, &mut connection_manager.events);
                                            // }
                                        });
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
    // // finalize any packets that are needed for replication
    // connection_manager
    //     .buffer_replication_messages(tick_manager.tick(), change_tick.this_run())
    //     .unwrap_or_else(|e| {
    //         error!("Error preparing replicate send: {}", e);
    //     });

    // SEND_PACKETS: send buffered packets to io
    let span = trace_span!("send_packets").entered();
    connection_manager
        .connections
        .iter_mut()
        .try_for_each(|(client_id, connection)| {
            let client_span =
                trace_span!("send_packets_to_client", client_id = ?client_id).entered();
            let netserver_idx = *netservers
                .client_server_map
                .get(client_id)
                .context("could not find server connection corresponding to client id")?;
            let netserver = netservers
                .servers
                .get_mut(netserver_idx)
                .context("could not find server with the provided netserver idx")?;
            for packet_byte in connection.send_packets(&time_manager, &tick_manager)? {
                netserver.send(packet_byte.as_slice(), *client_id)?;
            }
            Ok(())
        })
        .unwrap_or_else(|e: anyhow::Error| {
            error!("Error sending packets: {}", e);
        });

    // clear the list of newly connected clients
    // (cannot just use the ConnectionEvent because it is cleared after each frame)
    connection_manager.new_clients.clear();
}

/// Clear the received events
/// We put this in a separate system as send because we want to run this every frame, and
/// Send only runs every send_interval
pub(crate) fn clear_events<P: Protocol>(mut connection_manager: ResMut<ConnectionManager>) {
    // connection_manager.events.clear();
}

/// Run condition to check that the server is ready to send packets
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub(crate) fn is_started(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(false, |s| s.is_listening())
}

/// Run condition to check that the server is stopped.
///
/// We check the status of the `ServerConnections` directly instead of using the `State<NetworkingState>`
/// to avoid having a frame of delay since the `StateTransition` schedule runs after the `PreUpdate` schedule
pub(crate) fn is_stopped(server: Option<Res<ServerConnections>>) -> bool {
    server.map_or(true, |s| !s.is_listening())
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
fn rebuild_server_connections<P: Protocol>(world: &mut World) {
    let server_config = world.resource::<ServerConfig>().clone();

    // insert a new connection manager (to reset message numbers, ping manager, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<P>().message_registry().clone(),
        world.resource::<P>().channel_registry().clone(),
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
fn on_start<P: Protocol>(world: &mut World) {
    if world.resource::<ServerConnections>().is_listening() {
        error!("The server is already started. The server can only be started when it is stopped.");
        return;
    }
    rebuild_server_connections::<P>(world);
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
