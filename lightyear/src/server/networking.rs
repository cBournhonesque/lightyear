//! Defines the server bevy systems and run conditions
use crate::connection::server::{IoConfig, NetServer, ServerConnection, ServerConnections};
use crate::prelude::{
    is_host_server, server::is_started, ChannelRegistry, MainSet, MessageRegistry, TickManager,
    TimeManager,
};
use crate::protocol::component::ComponentRegistry;
use crate::serialize::reader::Reader;
use crate::server::clients::ControlledEntities;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::error::ServerError;
use crate::server::io::ServerIoEvent;
use crate::shared::sets::{InternalMainSet, ServerMarker};
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
                InternalMainSet::<ServerMarker>::Send.in_set(MainSet::Send),
            )
            // SYSTEMS //
            .add_systems(
                PreUpdate,
                (receive_packets, receive)
                    .chain()
                    .in_set(InternalMainSet::<ServerMarker>::Receive),
            )
            .add_systems(
                PostUpdate,
                (send, send_host_server.run_if(is_host_server))
                    .in_set(InternalMainSet::<ServerMarker>::Send),
            );

        // ON_START
        app.add_systems(OnEnter(NetworkingState::Started), on_start);

        // ON_STOP
        app.add_systems(OnEnter(NetworkingState::Stopped), on_stop);
    }

    // This runs after all plugins have run build() and finish()
    // so we are sure that the ComponentRegistry/MessageRegistry have been built
    fn cleanup(&self, app: &mut App) {
        // TODO: update all systems that need these to only run when needed, so that we don't have to create
        //  a ConnectionManager or a NetConfig at startup
        // Create the server connection resources to avoid some systems panicking
        // TODO: remove this when possible?
        app.world_mut().run_system_once(rebuild_server_connections);
    }
}

pub(crate) fn receive_packets(
    mut commands: Commands,
    mut connection_manager: ResMut<ConnectionManager>,
    mut networking_state: ResMut<NextState<NetworkingState>>,
    mut netservers: ResMut<ServerConnections>,
    mut time_manager: ResMut<TimeManager>,
    tick_manager: Res<TickManager>,
    virtual_time: Res<Time<Virtual>>,
    component_registry: Res<ComponentRegistry>,
    message_registry: Res<MessageRegistry>,
    system_change_tick: SystemChangeTick,
) {
    trace!("Receive client packets");
    let delta = virtual_time.delta();
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
                                networking_state.set(NetworkingState::Stopped);
                            }
                            _ => {}
                        }
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Closed) => {}
                }
            }
        }

        // copy the disconnections here because they get cleared in `netserver.try_update`
        let new_disconnections = netserver.new_disconnections();
        let _ = netserver
            .try_update(delta.as_secs_f64())
            .map_err(|e| error!("Error updating netcode server: {:?}", e));
        for client_id in netserver.new_connections().iter().copied() {
            netservers.client_server_map.insert(client_id, server_idx);
            // spawn an entity for the client
            let client_entity = commands
                .spawn((ControlledEntities::default(), Name::new("Client")))
                .id();
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
        for client_id in new_disconnections {
            if netservers.client_server_map.remove(&client_id).is_some() {
                connection_manager.remove(client_id);
                // NOTE: we don't despawn the entity right away to let the user react to
                // the disconnect event
                // TODO: use observers/component_hooks to react automatically on the client despawn?
                // world.despawn(client_entity);
            } else {
                error!("Client disconnected but could not map client_id to the corresponding netserver");
            }
        }
    }

    // update connections
    connection_manager.update(
        system_change_tick.this_run(),
        time_manager.as_ref(),
        tick_manager.as_ref(),
    );

    // RECV_PACKETS: buffer packets into message managers
    // enable split borrows on connection manager
    let connection_manager = &mut *connection_manager;
    for (server_idx, netserver) in netservers.servers.iter_mut().enumerate() {
        while let Some((payload, client_id)) = netserver.recv() {
            // Note: the client_id might not be present in the connection_manager if we receive
            // packets from a client
            // TODO: use connection to apply on BOTH message manager and replication manager
            if let Some(connection) = connection_manager.connections.get_mut(&client_id) {
                connection
                    .recv_packet(
                        payload,
                        tick_manager.as_ref(),
                        component_registry.as_ref(),
                        &mut connection_manager.delta_manager,
                    )
                    .expect("could not receive packet");
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
}

/// Read from internal buffers and apply the changes to the world
pub(crate) fn receive(
    world: &mut World,
    // component_registry: Res<ComponentRegistry>,
    // message_registry: Res<MessageRegistry>,
    // time_manager: Res<TimeManager>,
    // tick_manager: Res<TickManager>,
) {
    let unsafe_world = world.as_unsafe_world_cell();

    // TODO: an alternative would be to use `Commands + EntityMut` which both don't conflict with resources
    // SAFETY: we guarantee that the `world` is not used in `connection_manager.receive` to update
    //  these resources
    let mut connection_manager =
        unsafe { unsafe_world.get_resource_mut::<ConnectionManager>() }.unwrap();
    let component_registry = unsafe { unsafe_world.get_resource::<ComponentRegistry>() }.unwrap();
    let message_registry = unsafe { unsafe_world.get_resource::<MessageRegistry>() }.unwrap();
    let time_manager = unsafe { unsafe_world.get_resource::<TimeManager>() }.unwrap();
    let tick_manager = unsafe { unsafe_world.get_resource::<TickManager>() }.unwrap();
    // RECEIVE: read messages and parse them into events
    connection_manager
        .receive(
            unsafe { unsafe_world.world_mut() },
            component_registry,
            message_registry,
            time_manager,
            tick_manager,
        )
        .unwrap_or_else(|e| {
            error!("Error during receive: {}", e);
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
    // SEND_PACKETS: send buffered packets to io
    let span = info_span!("send_packets").entered();
    connection_manager
        .connections
        .iter_mut()
        .filter(|(_, connection)| !connection.is_local_client())
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
}

/// When running in host-server mode, we also need to send messages to the local client.
/// We do this directly without io.
pub(crate) fn send_host_server(
    mut connection_manager: ResMut<ConnectionManager>,
    mut client_manager: ResMut<crate::client::connection::ConnectionManager>,
) {
    let _ = connection_manager
        .connections
        .iter_mut()
        .filter(|(_, connection)| connection.is_local_client())
        .try_for_each(|(_, connection)| {
            connection
                .local_messages_to_send
                .drain(..)
                .try_for_each(|message| client_manager.receive_message(Reader::from(message)))
        })
        .inspect_err(|e| error!("Error sending messages to local client: {:?}", e));
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
    debug!("Rebuild server connection");
    let server_config = world.resource::<ServerConfig>().clone();

    // insert a new connection manager (to reset message numbers, ping manager, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<MessageRegistry>().clone(),
        world.resource::<ChannelRegistry>().clone(),
        server_config.replication,
        server_config.packet,
        server_config.ping,
    );
    // // make sure the previous replication metadata is ported over to the new manager
    // if let Some(mut previous_manager) = world.get_resource_mut::<ConnectionManager>() {
    //     connection_manager.replicate_component_cache =
    //         std::mem::take(&mut previous_manager.replicate_component_cache);
    // }
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
    info!("Server is started.");
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
        self.insert_resource(NextState::Pending(NetworkingState::Started));
    }

    fn stop_server(&mut self) {
        self.insert_resource(NextState::Pending(NetworkingState::Stopped));
    }
}
