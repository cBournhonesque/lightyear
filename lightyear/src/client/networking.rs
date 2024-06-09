//! Defines the plugin related to the client networking (sending and receiving packets).
use std::ops::DerefMut;

use async_channel::TryRecvError;
use bevy::ecs::system::{RunSystemOnce, SystemChangeTick};
use bevy::prelude::ResMut;
use bevy::prelude::*;
use tracing::{error, trace};

use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::{ConnectEvent, DisconnectEvent};
use crate::client::interpolation::Interpolated;
use crate::client::io::ClientIoEvent;
use crate::client::networking::utils::AppStateExt;
use crate::client::prediction::Predicted;
use crate::client::replication::send::ReplicateToServer;
use crate::client::sync::SyncSet;
use crate::connection::client::{ClientConnection, ConnectionState, DisconnectReason, NetClient};
use crate::connection::server::IoConfig;
use crate::prelude::{
    is_host_server, ChannelRegistry, MainSet, MessageRegistry, TickManager, TimeManager,
};
use crate::protocol::component::ComponentRegistry;
use crate::server::clients::ControlledEntities;
use crate::shared::config::Mode;
use crate::shared::replication::components::Replicated;
use crate::shared::run_conditions;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use crate::shared::tick_manager::TickEvent;
use crate::transport::io::IoState;

#[derive(Default)]
pub(crate) struct ClientNetworkingPlugin;

impl Plugin for ClientNetworkingPlugin {
    fn build(&self, app: &mut App) {
        app
            // REFLECTION
            .register_type::<HostServerMetadata>()
            .register_type::<IoConfig>()
            // STATE
            .init_state_without_entering::<NetworkingState>()
            // RESOURCE
            .init_resource::<HostServerMetadata>()
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                (
                    InternalMainSet::<ClientMarker>::Receive.in_set(MainSet::Receive),
                    InternalMainSet::<ClientMarker>::EmitEvents.in_set(MainSet::EmitEvents),
                )
                    .chain()
                    .run_if(not(is_host_server.or_else(run_conditions::is_disconnected))),
            )
            .configure_sets(
                PostUpdate,
                // run sync before send because some send systems need to know if the client is synced
                // we don't send packets every frame, but on a timer instead
                (
                    SyncSet,
                    InternalMainSet::<ClientMarker>::Send.in_set(MainSet::Send),
                )
                    .run_if(not(is_host_server.or_else(run_conditions::is_disconnected)))
                    .chain(),
            )
            // SYSTEMS
            .add_systems(
                PreUpdate,
                listen_io_state
                    // we are running the listen_io_state in a different set because it can impact the run_condition for the
                    // Receive system set
                    .before(InternalMainSet::<ClientMarker>::Receive)
                    .run_if(not(is_host_server.or_else(run_conditions::is_disconnected))),
            )
            .add_systems(
                PreUpdate,
                (listen_io_state, receive).in_set(InternalMainSet::<ClientMarker>::Receive),
            )
            .add_systems(
                PostUpdate,
                (
                    send.in_set(InternalMainSet::<ClientMarker>::Send),
                    // TODO: update virtual time with Time<Real> so we have more accurate time at Send time.
                    sync_update.in_set(SyncSet),
                ),
            );

        // STARTUP
        // TODO: update all systems that need these to only run when needed, so that we don't have to create
        //  a ConnectionManager or a NetConfig at startup
        // Create a new `ClientConnection` and `ConnectionManager` at startup, so that systems
        // that depend on these resources do not panic
        app.world.run_system_once(rebuild_client_connection);

        // CONNECTING
        app.add_systems(OnEnter(NetworkingState::Connecting), connect);

        // CONNECTED
        app.add_systems(
            OnEnter(NetworkingState::Connected),
            (on_connect, on_connect_host_server.run_if(is_host_server)),
        );

        // DISCONNECTED
        app.add_systems(
            OnEnter(NetworkingState::Disconnected),
            (
                on_disconnect,
                on_disconnect_host_server.run_if(is_host_server),
            ),
        );
    }
}

pub(crate) fn receive(world: &mut World) {
    trace!("Receive server packets");
    world.resource_scope(
        |world: &mut World, mut connection: Mut<ConnectionManager>| {
            world.resource_scope(
                |world: &mut World, mut netclient: Mut<ClientConnection>| {
                        world.resource_scope(
                            |world: &mut World, mut time_manager: Mut<TimeManager>| {
                                world.resource_scope(
                                    |world: &mut World, tick_manager: Mut<TickManager>| {
                                        world.resource_scope(
                                            |world: &mut World, state: Mut<State<NetworkingState>>| {
                                                world.resource_scope(
                                                    |world: &mut World, mut next_state: Mut<NextState<NetworkingState>>| {
                                                        let delta = world.resource::<Time<Virtual>>().delta();
                                                        // UPDATE: update client state, send keep-alives, receive packets from io, update connection sync state
                                                        time_manager.update(delta);
                                                        trace!(time = ?time_manager.current_time(), tick = ?tick_manager.tick(), "receive");

                                                        if !matches!(netclient.state(), ConnectionState::Disconnected {..}){
                                                            let _ = netclient
                                                                .try_update(delta.as_secs_f64())
                                                                .map_err(|e| {
                                                                    error!("Error updating netcode: {}", e);
                                                                });
                                                        }

                                                        if matches!(netclient.state(), ConnectionState::Connected) {
                                                            // we just connected, do a state transition
                                                            if state.get() != &NetworkingState::Connected {
                                                                debug!("Setting the networking state to connected");
                                                                next_state.set(NetworkingState::Connected);
                                                            }

                                                            // update the connection (message manager, ping manager, etc.)
                                                            connection.update(
                                                                world.change_tick(),
                                                                time_manager.as_ref(),
                                                                tick_manager.as_ref(),
                                                            );
                                                        }
                                                        if let ConnectionState::Disconnected{reason} = netclient.state() {
                                                            netclient.disconnect_reason = reason;
                                                            // we just disconnected, do a state transition
                                                            if state.get() != &NetworkingState::Disconnected {
                                                                next_state.set(NetworkingState::Disconnected);
                                                            }
                                                        }

                                                        // RECV PACKETS: buffer packets into message managers
                                                        while let Some(packet) = netclient.recv() {
                                                            connection
                                                                .recv_packet(packet, tick_manager.as_ref(), world.resource::<ComponentRegistry>())
                                                                .unwrap();
                                                        }
                                                        // RECEIVE: receive packets from message managers
                                                        let _ = connection.receive(world, time_manager.as_ref(), tick_manager.as_ref()).inspect_err(|e| error!("Error receiving packets: {}", e));
                                                    });
                                            });
                                        });
                                    },
                                )
                            }
                    );
                }
            );
    trace!("client finished recv");
}

pub(crate) fn send(
    mut netcode: ResMut<ClientConnection>,
    system_change_tick: SystemChangeTick,
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
    mut connection: ResMut<ConnectionManager>,
) {
    trace!("Send packets to server");
    // finalize any packets that are needed for replication
    connection
        .buffer_replication_messages(tick_manager.tick(), system_change_tick.this_run())
        .unwrap_or_else(|e| {
            error!("Error preparing replicate send: {}", e);
        });
    // SEND_PACKETS: send buffered packets to io
    let packet_bytes = connection
        .send_packets(time_manager.as_ref(), tick_manager.as_ref())
        .unwrap();
    for packet_byte in packet_bytes {
        let _ = netcode.send(packet_byte.as_slice()).map_err(|e| {
            error!("Error sending packet: {}", e);
        });
    }

    // no need to clear the connection, because we already std::mem::take it
    // client.connection.clear();
}

/// Update the sync manager.
/// We run this at PostUpdate because:
/// - client prediction time is computed from ticks, which haven't been updated yet at PreUpdate
/// - server prediction time is computed from time, which has been updated via delta
/// Also server sends the tick after FixedUpdate, so it makes sense that we would compare to the client tick after FixedUpdate
/// So instead we update the sync manager at PostUpdate, after both ticks/time have been updated
pub(crate) fn sync_update(
    config: Res<ClientConfig>,
    netclient: Res<ClientConnection>,
    connection: ResMut<ConnectionManager>,
    mut time_manager: ResMut<TimeManager>,
    mut tick_manager: ResMut<TickManager>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut tick_events: EventWriter<TickEvent>,
) {
    let connection = connection.into_inner();
    // NOTE: this triggers change detection
    // Handle pongs, update RTT estimates, update client prediction time
    if let Some(tick_event) = connection.sync_manager.update(
        time_manager.deref_mut(),
        tick_manager.deref_mut(),
        &connection.ping_manager,
        &config.interpolation.delay,
        config.shared.server_send_interval,
    ) {
        tick_events.send(tick_event);
    }

    if connection.sync_manager.is_synced() {
        if let Some(tick_event) = connection.sync_manager.update_prediction_time(
            time_manager.deref_mut(),
            tick_manager.deref_mut(),
            &connection.ping_manager,
        ) {
            tick_events.send(tick_event);
        }
        let relative_speed = time_manager.get_relative_speed();
        virtual_time.set_relative_speed(relative_speed);
    }
}

/// Bevy [`State`] representing the networking state of the client.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkingState {
    /// The client is disconnected from the server. The receive/send packets systems do not run.
    #[default]
    Disconnected,
    /// The client is trying to connect to the server
    Connecting,
    /// The client is connected to the server
    Connected,
}

/// Listen to [`ClientIoEvent`]s and update the [`IoState`] and [`NetworkingState`] accordingly
fn listen_io_state(
    mut next_state: ResMut<NextState<NetworkingState>>,
    mut netclient: ResMut<ClientConnection>,
) {
    let mut disconnect = false;
    if let Some(io) = netclient.io_mut() {
        if let Some(receiver) = io.context.event_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(ClientIoEvent::Connected) => {
                    debug!("Io is connected!");
                    io.state = IoState::Connected;
                }
                Ok(ClientIoEvent::Disconnected(e)) => {
                    error!("Error from io: {}", e);
                    io.state = IoState::Disconnected;
                    netclient.disconnect_reason = Some(DisconnectReason::Transport(e));
                    disconnect = true;
                }
                Err(TryRecvError::Empty) => {
                    trace!("we are still connecting the io, and there is no error yet");
                }
                Err(TryRecvError::Closed) => {
                    error!("Io status channel has been closed when it shouldn't be");
                    netclient.disconnect_reason = Some(DisconnectReason::Transport(
                        std::io::Error::other("Io status channel has been closed").into(),
                    ));
                    disconnect = true;
                }
            }
        }
    }
    if disconnect {
        debug!("Going to NetworkingState::Disconnected because of io error.");
        next_state.set(NetworkingState::Disconnected);
        // TODO: do we need to disconnect here? we disconnect in the OnEnter(Disconnected) system anyway
        let _ = netclient
            .disconnect()
            .inspect_err(|e| debug!("error disconnecting netclient: {e:?}"));
    }
}

/// Holds metadata necessary when running in HostServer mode
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
struct HostServerMetadata {
    /// entity for the client running as host-server
    client_entity: Option<Entity>,
}

/// System that runs when we enter the Connected state
/// Updates the ConnectEvent events
fn on_connect(
    mut connect_event_writer: EventWriter<ConnectEvent>,
    netcode: Res<ClientConnection>,
    mut query: Query<&mut ReplicateToServer>,
) {
    // Set all the ReplicateToServer ticks to changed, so that we replicate existing entities to the server
    for mut replicate in query.iter_mut() {
        // TODO: ideally set is_added instead of simply changed
        replicate.set_changed();
    }
    debug!(
        "Running OnConnect schedule with client id: {:?}",
        netcode.id()
    );
    connect_event_writer.send(ConnectEvent::new(netcode.id()));
}

/// Same as on-connect, but only runs if we are in host-server mode
fn on_connect_host_server(
    mut commands: Commands,
    netcode: Res<ClientConnection>,
    mut metadata: ResMut<HostServerMetadata>,
    mut server_connect_event_writer: ResMut<Events<crate::server::events::ConnectEvent>>,
) {
    // spawn an entity for the client
    let client_entity = commands.spawn(ControlledEntities::default()).id();
    server_connect_event_writer.send(crate::server::events::ConnectEvent {
        client_id: netcode.id(),
        entity: client_entity,
    });
    metadata.client_entity = Some(client_entity);
}

/// System that runs when we enter the Disconnected state
/// Updates the DisconnectEvent events
fn on_disconnect(
    mut connection_manager: ResMut<ConnectionManager>,
    mut disconnect_event_writer: EventWriter<DisconnectEvent>,
    mut netclient: ResMut<ClientConnection>,
    mut commands: Commands,
    received_entities: Query<Entity, Or<(With<Replicated>, With<Predicted>, With<Interpolated>)>>,
) {
    info!("Running OnDisconnect schedule");
    // despawn any entities that were spawned from replication
    received_entities
        .iter()
        .for_each(|e| commands.entity(e).despawn_recursive());

    // set synced to false
    connection_manager.sync_manager.synced = false;

    // try to disconnect again to close io tasks (in case the disconnection is from the io)
    let _ = netclient.disconnect();

    // no need to update the io state, because we will recreate a new `ClientConnection`
    // for the next connection attempt
    let reason = std::mem::take(&mut netclient.disconnect_reason);
    disconnect_event_writer.send(DisconnectEvent { reason });
    // TODO: remove ClientConnection and ConnectionManager resources?
}

fn on_disconnect_host_server(
    netcode: Res<ClientConnection>,
    mut metadata: ResMut<HostServerMetadata>,
    mut server_disconnect_event_writer: ResMut<Events<crate::server::events::DisconnectEvent>>,
) {
    let client_id = netcode.id();
    if let Some(client_entity) = std::mem::take(&mut metadata.client_entity) {
        server_disconnect_event_writer.send(crate::server::events::DisconnectEvent {
            client_id,
            entity: client_entity,
        });
    }
}

/// This runs only when we enter the [`Connecting`](NetworkingState::Connecting) state.
///
/// We rebuild the [`ClientConnection`] by using the latest [`ClientConfig`].
/// This has several benefits:
/// - the client connection's internal time is up-to-date (otherwise it might not be, since we don't call `update` while disconnected)
/// - we can take into account any changes to the client config
fn rebuild_client_connection(world: &mut World) {
    let client_config = world.resource::<ClientConfig>().clone();
    // if client_config.shared.mode == Mode::HostServer {
    //     assert!(
    //         matches!(client_config.net, NetConfig::Local { .. }),
    //         "When running in HostServer mode, the client connection needs to be of type Local"
    //     );
    // }

    // insert a new connection manager (to reset sync, priority, message numbers, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<ComponentRegistry>(),
        world.resource::<MessageRegistry>(),
        world.resource::<ChannelRegistry>(),
        client_config.replication,
        client_config.packet,
        client_config.sync,
        client_config.ping,
        client_config.prediction.input_delay_ticks,
    );
    world.insert_resource(connection_manager);

    // drop the previous client connection to make sure we release any resources before creating the new one
    world.remove_resource::<ClientConnection>();
    // insert the new client connection
    let client_connection = client_config.net.build_client();
    world.insert_resource(client_connection);
}

// TODO: the design where the user has to call world.connect_client() is better because the user can handle the Error however they want!

/// Connect the client
/// - rebuild the client connection resource using the latest `ClientConfig`
/// - rebuild the client connection manager
/// - start the connection process
/// - set the networking state to `Connecting`
fn connect(world: &mut World) {
    // TODO: should we prevent running Connect if we're already Connected?
    // if world.resource::<ClientConnection>().state() == NetworkingState::Connected {
    //     error!("The client is already started. The client can only start connecting when it is disconnected.");
    // }

    // Everytime we try to connect, we rebuild the net config because:
    // - we do not call update() while the client is disconnected, so the internal connection's time is wrong
    // - this allows us to take into account any changes to the client config (when building a
    // new client connection and connection manager, which want to do because we need to reset
    // the internal time, sync, priority, message numbers, etc.)
    rebuild_client_connection(world);
    let _ = world
        .resource_mut::<ClientConnection>()
        .connect()
        .inspect_err(|e| {
            error!("Error connecting client: {}", e);
        });
    let config = world.resource::<ClientConfig>();

    if matches!(
        world.resource::<ClientConnection>().state(),
        ConnectionState::Connected
    ) && config.shared.mode == Mode::HostServer
    {
        // TODO: also check if the connection is of type local?
        // in host server mode, there is no connecting phase, we directly become connected
        // (because the networking systems don't run so we cannot go through the Connecting state)
        world
            .resource_mut::<NextState<NetworkingState>>()
            .set(NetworkingState::Connected);
    }
}

pub trait ClientCommands {
    /// Start the connection process
    fn connect_client(&mut self);

    /// Disconnect the client
    fn disconnect_client(&mut self);
}

impl ClientCommands for Commands<'_, '_> {
    fn connect_client(&mut self) {
        self.insert_resource(NextState::<NetworkingState>(Some(
            NetworkingState::Connecting,
        )));
    }

    fn disconnect_client(&mut self) {
        self.insert_resource(NextState::<NetworkingState>(Some(
            NetworkingState::Disconnected,
        )));
    }
}

mod utils {
    use bevy::app::{App, StateTransition};
    use bevy::prelude::{
        apply_state_transition, FromWorld, NextState, State, StateTransitionEvent, States,
    };

    pub(super) trait AppStateExt {
        // Helper function that runs `init_state::<S>` without entering the state
        // This is useful for us as we don't want to run OnEnter<NetworkingState::Disconnected> when we start the app
        fn init_state_without_entering<S: States + FromWorld>(&mut self) -> &mut Self;
    }

    impl AppStateExt for App {
        fn init_state_without_entering<S: States + FromWorld>(&mut self) -> &mut Self {
            if !self.world.contains_resource::<State<S>>() {
                self.init_resource::<State<S>>()
                    .init_resource::<NextState<S>>()
                    .add_event::<StateTransitionEvent<S>>()
                    .add_systems(StateTransition, apply_state_transition::<S>);
            }

            // The OnEnter, OnExit, and OnTransition schedules are lazily initialized
            // (i.e. when the first system is added to them), and World::try_run_schedule is used to fail
            // gracefully if they aren't present.
            self
        }
    }
}
