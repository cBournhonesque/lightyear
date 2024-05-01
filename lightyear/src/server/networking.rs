//! Defines the server bevy systems and run conditions
use anyhow::{anyhow, Context};
use bevy::ecs::system::{RunSystemOnce, SystemChangeTick, SystemParam};
use bevy::prelude::*;
use tracing::{debug, error, trace, trace_span};

use crate::client::config::ClientConfig;
use crate::client::networking::is_disconnected;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::server::{NetConfig, NetServer, ServerConnection, ServerConnections};
use crate::prelude::{ChannelRegistry, MainSet, MessageRegistry, Mode, TickManager, TimeManager};
use crate::protocol::component::ComponentRegistry;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::server::room::RoomManager;
use crate::shared::events::connection::{IterEntityDespawnEvent, IterEntitySpawnEvent};
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use crate::shared::time_manager::is_server_ready_to_send;

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
                                    world.resource_scope(
                                        |world: &mut World, mut room_manager: Mut<RoomManager>| {
                                            let delta = world.resource::<Time<Virtual>>().delta();
                                            // UPDATE: update server state, send keep-alives, receive packets from io
                                            // update time manager
                                            time_manager.update(delta);
                                            trace!(time = ?time_manager.current_time(), tick = ?tick_manager.tick(), "receive");

                                            // update connections
                                            connection_manager
                                                .update(time_manager.as_ref(), tick_manager.as_ref());

                                            let servers = world.query::<(Entity, &mut ServerConnection)>().par_iter_mut(world);
                                            servers.for_each(|(entity, mut server)| {
                                                // Update the server state
                                                let _ = server
                                                    .try_update(delta.as_secs_f64())
                                                    .map_err(|e| error!("Error updating netcode server: {:?}", e));
                                                for client_id in server.new_connections().iter().copied() {
                                                    netservers.client_server_map.insert(client_id, entity);
                                                    connection_manager.add(client_id);
                                                }
                                                // handle disconnections
                                                for client_id in server.new_disconnections().iter().copied() {
                                                    if netservers.client_server_map.remove(&client_id).is_some() {
                                                        connection_manager.remove(client_id);
                                                        room_manager.client_disconnect(client_id);
                                                    } else {
                                                        error!("Client disconnected but could not map client_id to the corresponding netserver");
                                                    }
                                                };

                                                // RECV_PACKETS: buffer packets into message managers
                                                while let Some((packet, client_id)) = server.recv() {
                                                    // Note: the client_id might not be present in the connection_manager if we receive
                                                    // packets from a client
                                                    // TODO: use connection to apply on BOTH message manager and replication manager
                                                    if let Ok(connection) = connection_manager
                                                        .connection_mut(client_id) {
                                                        connection.recv_packet(packet, tick_manager.as_ref()).expect("could not receive packet");
                                                    } else {
                                                        // it's still possible to receive some packets from a client that just disconnected.
                                                        // (multiple packets arrived at the same time from that client)
                                                        if server.new_disconnections().contains(&client_id) {
                                                            trace!("received packet from client that just got disconnected. Ignoring.");
                                                            // we ignore packets from disconnected clients
                                                            // this is not an error
                                                            continue;
                                                        } else {
                                                            error!("Received packet from unknown client: {}", client_id);
                                                        }
                                                    }
                                                }
                                            });

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
                                                    for client_id in connection_manager.events.iter_connections() {
                                                        debug!("Client connected event: {}", client_id);
                                                        connect_event_writer.send(ConnectEvent::new(client_id));
                                                    }
                                                }

                                                if connection_manager.events.has_disconnections() {
                                                    let mut connect_event_writer =
                                                        world.get_resource_mut::<Events<DisconnectEvent>>().unwrap();
                                                    for client_id in connection_manager.events.iter_disconnections() {
                                                        debug!("Client disconnected event: {}", client_id);
                                                        connect_event_writer.send(DisconnectEvent::new(client_id));
                                                    }
                                                }
                                            }
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
    mut servers: Query<&mut ServerConnection>,
) {
    trace!("Send packets to clients");
    // finalize any packets that are needed for replication
    connection_manager
        .buffer_replication_messages(tick_manager.tick(), change_tick.this_run())
        .unwrap_or_else(|e| {
            error!("Error preparing replicate send: {}", e);
        });

    // SEND_PACKETS: send buffered packets to io
    let span = trace_span!("send_packets").entered();
    connection_manager
        .connections
        .iter_mut()
        .try_for_each(|(client_id, connection)| {
            let client_span =
                trace_span!("send_packets_to_client", client_id = ?client_id).entered();
            let server_entity = *netservers
                .client_server_map
                .get(client_id)
                .context("could not find server connection corresponding to client id")?;
            let mut server = servers
                .get_mut(server_entity)
                .context("could not find server with the provided netserver idx")?;
            for packet_byte in connection.send_packets(&time_manager, &tick_manager)? {
                server.send(packet_byte.as_slice(), *client_id)?;
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
fn rebuild_server_connections(world: &mut World) {
    let server_config = world.resource::<ServerConfig>().clone();

    // insert a new connection manager (to reset message numbers, ping manager, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<ComponentRegistry>().clone(),
        world.resource::<MessageRegistry>().clone(),
        world.resource::<ChannelRegistry>().clone(),
        server_config.packet,
        server_config.ping,
    );
    world.insert_resource(connection_manager);

    // rebuild the server connections and insert them
    world.insert_resource(ServerConnections::default());
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
    world.resource_scope(
        |world: &mut World, mut connections: Mut<ServerConnections>| {
            let _ = connections
                .start(world)
                .inpect_err(|e| error!("Error starting server connections: {:?}", e));
        },
    );
}

/// System that runs when we enter the Stopped state
fn on_stop(world: &mut World) {
    world.resource_scope(
        |world: &mut World, mut connections: Mut<ServerConnections>| {
            let _ = connections
                .stop(world)
                .inspect_err(|e| error!("Error stopping server connections: {:?}", e));
        },
    );
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
