//! Defines the plugin related to the client networking (sending and receiving packets).
use async_channel::TryRecvError;
use bevy::ecs::system::{RunSystemOnce, SystemChangeTick};
use bevy::prelude::*;
use core::ops::DerefMut;
use core::time::Duration;
#[cfg(feature = "std")]
use std::io;
use tracing::{debug, error, trace};
#[cfg(not(feature = "std"))]
use no_std_io2::io;

use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::{ConnectEvent, DisconnectEvent};
use crate::client::io::ClientIoEvent;
use crate::client::replication::send::ReplicateToServer;
use crate::client::run_conditions::is_disconnected;
use crate::client::sync::SyncSet;
use crate::connection::client::{ClientConnection, ConnectionError, ConnectionState, NetClient};
use crate::connection::server::IoConfig;
use crate::prelude::client::NetConfig;
use crate::prelude::{
    is_host_server, server, ChannelRegistry, ComponentRegistry, MainSet, MessageRegistry,
    TickManager, TimeManager,
};
use crate::server::clients::ControlledEntities;
use crate::shared::replication::components::Replicated;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use crate::transport::io::IoState;

#[derive(Default)]
pub(crate) struct ClientNetworkingPlugin;

impl Plugin for ClientNetworkingPlugin {
    fn build(&self, app: &mut App) {
        app
            // REFLECTION
            .register_type::<HostServerMetadata>()
            .register_type::<IoConfig>()
            // RESOURCE
            .init_resource::<HostServerMetadata>()
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                (
                    InternalMainSet::<ClientMarker>::Receive
                        .in_set(MainSet::Receive)
                        // do not receive packets when running in host-server mode
                        // the messages from the server will be transmitted directly
                        .run_if(not(is_host_server)),
                    // we still want to emit events when running in host-server mode
                    InternalMainSet::<ClientMarker>::ReceiveEvents.in_set(MainSet::ReceiveEvents),
                )
                    .chain()
                    .run_if(not(is_disconnected)),
            )
            .configure_sets(
                PostUpdate,
                // run sync before send because some send systems need to know if the client is synced
                // we don't send packets every frame, but on a timer instead
                (
                    SyncSet.run_if(not(is_host_server)),
                    InternalMainSet::<ClientMarker>::Send.in_set(MainSet::Send),
                )
                    .run_if(not(is_disconnected))
                    .chain(),
            )
            // SYSTEMS
            .add_systems(
                PreUpdate,
                (listen_io_state, (receive_packets, receive).chain())
                    .in_set(InternalMainSet::<ClientMarker>::Receive),
            )
            .add_systems(
                PostUpdate,
                (
                    (
                        send.run_if(not(is_host_server)),
                        send_host_server.run_if(is_host_server),
                    )
                        .in_set(InternalMainSet::<ClientMarker>::Send),
                    // TODO: update virtual time with Time<Real> so we have more accurate time at Send time.
                    sync_update.in_set(SyncSet),
                ),
            );

        // CONNECTING
        app.add_systems(OnEnter(NetworkingState::Connecting), connect);

        // CONNECTED
        app.add_systems(
            OnEnter(NetworkingState::Connected),
            (
                on_connect.run_if(not(is_host_server)),
                on_connect_host_server.run_if(is_host_server),
            ),
        );

        // DISCONNECTED
        app.add_systems(
            OnEnter(NetworkingState::Disconnecting),
            (
                on_disconnecting.run_if(not(is_host_server)),
                on_disconnecting_host_server.run_if(is_host_server),
            ),
        );
    }

    // This runs after all plugins have run build() and finish()
    // so we are sure that the ComponentRegistry has been built
    fn cleanup(&self, app: &mut App) {
        // TODO: update all systems that need these to only run when needed, so that we don't have to create
        //  a ConnectionManager or a NetConfig at startup
        // Create a new `ClientConnection` and `ConnectionManager` at startup, so that systems
        // that depend on these resources do not panic
        // We build it here so that it uses the latest Protocol
        let _ = app.world_mut().run_system_once(rebuild_client_connection);
    }
}

pub(crate) fn receive_packets(
    mut connection: ResMut<ConnectionManager>,
    state: Res<State<NetworkingState>>,
    mut next_state: ResMut<NextState<NetworkingState>>,
    mut netclient: ResMut<ClientConnection>,
    mut time_manager: ResMut<TimeManager>,
    tick_manager: Res<TickManager>,
    virtual_time: Res<Time<Virtual>>,
    component_registry: Res<ComponentRegistry>,
    message_registry: Res<MessageRegistry>,
    system_change_tick: SystemChangeTick,
) -> Result {
    trace!("Receive server packets");
    let delta = virtual_time.delta();
    // UPDATE: update client state, send keep-alives, receive packets from io, update connection sync state
    time_manager.update(delta);
    trace!(time = ?time_manager.current_time(), tick = ?tick_manager.tick(), "receive");

    if !matches!(netclient.state(), ConnectionState::Disconnected { .. }) {
        let _ = netclient.try_update(delta.as_secs_f64()).map_err(|e| {
            error!("Error updating netcode: {}", e);
        });
    }

    if matches!(netclient.state(), ConnectionState::Connected) {
        // we just connected, do a state transition
        if state.get() == &NetworkingState::Connecting {
            debug!("Setting the networking state to connected");
            next_state.set(NetworkingState::Connected);
        }

        // update the connection (message manager, ping manager, etc.)
        connection.update(
            system_change_tick.this_run(),
            time_manager.as_ref(),
            tick_manager.as_ref(),
        );
    }
    if let ConnectionState::Disconnected { reason } = netclient.state() {
        netclient.disconnect_reason = reason;
        // we just disconnected, do a state transition
        if state.get() != &NetworkingState::Disconnecting {
            next_state.set(NetworkingState::Disconnecting);
        }
    }

    // RECV PACKETS: buffer packets into message managers
    while let Some(packet) = netclient.recv() {
        // TODO: should we just ignore the erroring-packet instead of exiting the system? probably..
        connection
            .recv_packet(packet, tick_manager.as_ref(), component_registry.as_ref())?
    }
    Ok(())
}

/// Read from internal buffers and apply the changes to the world
pub(crate) fn receive(world: &mut World) -> Result {
    let unsafe_world = world.as_unsafe_world_cell();

    // TODO: an alternative would be to use `Commands + EntityMut` which both don't conflict with resources
    // SAFETY: we guarantee that the `world` is not used in `connection_manager.receive` to update
    //  these resources
    let mut connection_manager =
        unsafe { unsafe_world.get_resource_mut::<ConnectionManager>() }.unwrap();
    let mut component_registry =
        unsafe { unsafe_world.get_resource_mut::<ComponentRegistry>() }.unwrap();
    let time_manager = unsafe { unsafe_world.get_resource::<TimeManager>() }.unwrap();
    let tick_manager = unsafe { unsafe_world.get_resource::<TickManager>() }.unwrap();
    // RECEIVE: read messages and parse them into events
    connection_manager
        .receive(
            unsafe { unsafe_world.world_mut() },
            component_registry.as_mut(),
            time_manager,
            tick_manager,
        )
        .inspect_err(|e| error!("Error receiving packets: {}", e))?;
    Ok(())
}

pub(crate) fn send(
    mut netcode: ResMut<ClientConnection>,
    system_change_tick: SystemChangeTick,
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
    mut connection: ResMut<ConnectionManager>,
) -> Result {
    trace!("Send packets to server");
    // SEND_PACKETS: send buffered packets to io
    let packet_bytes = connection
        .send_packets(time_manager.as_ref(), tick_manager.as_ref())?;
    for packet_byte in packet_bytes {
        let _ = netcode.send(packet_byte.as_slice()).map_err(|e| {
            error!("Error sending packet: {}", e);
        });
    }

    // no need to clear the connection, because we already core::mem::take it
    // client.connection.clear();
    Ok(())
}

/// Send messages in host-server mode
/// We cannot use the normal `send` function because there is no IO available
pub(crate) fn send_host_server(
    netcode: Res<ClientConnection>,
    mut client_manager: ResMut<ConnectionManager>,
    mut server_manager: ResMut<crate::server::connection::ConnectionManager>,
) -> Result {
    client_manager
        .send_packets_host_server(netcode.id(), server_manager.as_mut())
        .inspect_err(|e| {
            error!(
                "Error sending messages from local client to server in host-server mode: {}",
                e
            )
        })?;
    Ok(())
}

/// Update the sync manager.
/// We run this at PostUpdate because:
/// - client prediction time is computed from ticks, which haven't been updated yet at PreUpdate
/// - server prediction time is computed from time, which has been updated via delta
///
/// Also, server sends the tick after FixedUpdate, so it makes sense that we would compare to the client tick after FixedUpdate
/// So instead we update the sync manager at PostUpdate, after both ticks/time have been updated
pub(crate) fn sync_update(
    mut commands: Commands,
    config: Res<ClientConfig>,
    netclient: Res<ClientConnection>,
    connection: ResMut<ConnectionManager>,
    mut time_manager: ResMut<TimeManager>,
    mut tick_manager: ResMut<TickManager>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    let connection = connection.into_inner();
    // NOTE: this triggers change detection
    // Handle pongs, update RTT estimates, update client prediction time
    if let Some(tick_event) = connection.sync_manager.update(
        time_manager.deref_mut(),
        tick_manager.deref_mut(),
        &connection.ping_manager,
        &config.prediction,
        &config.interpolation,
        // TODO: how to adjust this for replication groups that have a custom send_interval?
        config.shared.server_replication_send_interval,
    ) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("sync::resync_event").increment(1);
        }
        debug!("Triggering TickSync event: {tick_event:?}");
        commands.trigger(tick_event);
    }

    if connection.sync_manager.is_synced() {
        if let Some(tick_event) = connection.sync_manager.update_prediction_time(
            time_manager.deref_mut(),
            tick_manager.deref_mut(),
            &connection.ping_manager,
            &config.prediction,
        ) {
            debug!("Triggering TickSync event: {tick_event:?}");
            commands.trigger(tick_event);
        }
        let relative_speed = time_manager.get_relative_speed();
        virtual_time.set_relative_speed(relative_speed);
    }
}

/// Bevy [`State`] representing the networking state of the client.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum NetworkingState {
    // We need this State to have a 1 frame transition so that we can still be in Host-Server mode
    // while the client is disconnecting. Otherwise as soon as we're Disconnected we are not in
    // HostServer mode anymore
    Disconnecting,
    /// The client is disconnected from the server. The receive/send packets systems do not run.
    #[default]
    Disconnected,
    /// The client is trying to connect to the server
    Connecting,
    /// The client is connected to the server
    Connected,
}

// TODO: update this state based on the sync system!
#[derive(SubStates, Clone, PartialEq, Eq, Hash, Debug, Default)]
#[source(NetworkingState = NetworkingState::Connected)]
pub enum ConnectedState {
    #[default]
    Unsynced,
    Synced,
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
                    netclient.disconnect_reason = Some(ConnectionError::Transport(e));
                    disconnect = true;
                }
                Err(TryRecvError::Empty) => {
                    trace!("we are still connecting the io, and there is no error yet");
                }
                Err(TryRecvError::Closed) => {
                    error!("Io status channel has been closed when it shouldn't be");
                    netclient.disconnect_reason = Some(ConnectionError::Transport(
                        io::Error::other("Io status channel has been closed").into(),
                    ));
                    disconnect = true;
                }
            }
        }
    }
    if disconnect {
        debug!("Going to NetworkingState::Disconnecting because of io error.");
        next_state.set(NetworkingState::Disconnecting);
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
    mut commands: Commands,
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
    connect_event_writer.write(ConnectEvent::new(netcode.id()));
    // also trigger the event
    commands.trigger(ConnectEvent::new(netcode.id()));
}

/// Same as on-connect, but only runs if we are in host-server mode
fn on_connect_host_server(
    mut commands: Commands,
    netcode: Res<ClientConnection>,
    config: Res<ClientConfig>,
    mut connection_manager: ResMut<ConnectionManager>,
    mut metadata: ResMut<HostServerMetadata>,
    mut server_manager: ResMut<crate::server::connection::ConnectionManager>,
    mut connect_event_writer: EventWriter<ConnectEvent>,
) {
    // make sure the `is_synced` run conditions return true
    // TODO: for some reason, enabling this breaks a LOT of things. I think we use the `is_synced` condition
    //  too liberally in the code-base, normally it should just mean that SyncPlugin is done, but in a lot cases
    //  we use it to mean: 'client is connected'. Investigate why this breaks things.
    //  For example in host-server mode in AvianPhysics example, the ReplicationTarget component is not added
    //   on the entity!
    // connection_manager.sync_manager.synced = true;

    // TODO: put this elsewhere! Maybe the SyncPlugin should have a startup function that runs if in host-server?
    // set the input delay value (since SyncPlugin does not run)
    connection_manager.sync_manager.current_input_delay = config
        .prediction
        .input_delay_ticks(Duration::default(), config.shared.tick.tick_duration);
    // spawn an entity for the client
    let client_entity = commands.spawn(ControlledEntities::default()).id();
    // start a server connection for that client (which will also send a ConnectEvent on the server)
    server_manager.add(netcode.id(), client_entity);
    server_manager
        .connection_mut(netcode.id())
        .unwrap()
        .set_local_client();
    metadata.client_entity = Some(client_entity);
    connect_event_writer.write(ConnectEvent::new(netcode.id()));
    // also trigger the event
    commands.trigger(ConnectEvent::new(netcode.id()));
}

/// System that runs when we enter the Disconnected state
/// Updates the DisconnectEvent events
fn on_disconnecting(
    mut connection_manager: ResMut<ConnectionManager>,
    mut disconnect_event_writer: EventWriter<DisconnectEvent>,
    mut netclient: ResMut<ClientConnection>,
    mut commands: Commands,
    // no need to handle Predicted/Interpolated because there are separate systems that handle these
    received_entities: Query<Entity, With<Replicated>>,
    mut networking_state: ResMut<NextState<NetworkingState>>,
) {
    // despawn any entities that were spawned from replication
    received_entities.iter().for_each(|e| {
        if let Ok(mut commands) = commands.get_entity(e) {
            commands.despawn();
        }
    });

    // set synced to false
    connection_manager.sync_manager.synced = false;
    // try to disconnect again to close io tasks (in case the disconnection is from the io)
    let _ = netclient.disconnect();

    // no need to update the io state, because we will recreate a new `ClientConnection`
    // for the next connection attempt
    let reason = core::mem::take(&mut netclient.disconnect_reason);
    disconnect_event_writer.write(DisconnectEvent { reason });
    // TODO: how can we also provide a reason here? or do we even need to?
    // we need to also trigger the event because we sometimes react to it via observers
    commands.trigger(DisconnectEvent { reason: None });
    // TODO: remove ClientConnection and ConnectionManager resources?
    networking_state.set(NetworkingState::Disconnected);
}

/// Make sure that the DisconnectEvent is emitted even when the local client disconnects
fn on_disconnecting_host_server(
    netcode: Res<ClientConnection>,
    mut connection_manager: ResMut<crate::prelude::server::ConnectionManager>,
    mut metadata: ResMut<HostServerMetadata>,
    mut networking_state: ResMut<NextState<NetworkingState>>,
) {
    let client_id = netcode.id();
    if let Some(client_entity) = core::mem::take(&mut metadata.client_entity) {
        // removing the client from the server's connection list also emits the server DisconnectEvent
        connection_manager.remove(client_id);
    }
    networking_state.set(NetworkingState::Disconnected);
}

/// This runs only when we enter the [`Connecting`](NetworkingState::Connecting) state.
///
/// We rebuild the [`ClientConnection`] by using the latest [`ClientConfig`].
/// This has several benefits:
/// - the client connection's internal time is up-to-date (otherwise it might not be, since we don't call `update` while disconnected)
/// - we can take into account any changes to the client config
fn rebuild_client_connection(world: &mut World) -> Result {
    let client_config = world.resource::<ClientConfig>().clone();

    // if the server is started, that means we're planning to run in host-server mode
    if world
        .get_resource::<State<server::NetworkingState>>()
        .is_some_and(|s| s.get() == &server::NetworkingState::Started)
    {
        assert!(
            matches!(client_config.net, NetConfig::Local { .. }),
            "When running in HostServer mode, the client connection needs to be of type Local"
        );
    }

    // insert a new connection manager (to reset sync, priority, message numbers, etc.)
    let connection_manager = ConnectionManager::new(
        world.resource::<MessageRegistry>(),
        world.resource::<ChannelRegistry>(),
        &client_config,
    );
    world.insert_resource(connection_manager);

    // drop the previous client connection to make sure we release any resources before creating the new one
    world.remove_resource::<ClientConnection>();
    // insert the new client connection
    let client_connection = client_config.net.build_client()?;
    world.insert_resource(client_connection);
    Ok(())
}

// TODO: the design where the user has to call world.connect_client() is better because the user can handle the Error however they want!

/// Connect the client
/// - rebuild the client connection resource using the latest `ClientConfig`
/// - rebuild the client connection manager
/// - start the connection process
/// - set the networking state to `Connecting`
fn connect(world: &mut World) -> Result {
    // TODO: should we prevent running Connect if we're already Connected?
    // if world.resource::<ClientConnection>().state() == NetworkingState::Connected {
    //     error!("The client is already started. The client can only start connecting when it is disconnected.");
    // }

    // Everytime we try to connect, we rebuild the net config because:
    // - we do not call update() while the client is disconnected, so the internal connection's time is wrong
    // - this allows us to take into account any changes to the client config (when building a
    // new client connection and connection manager, which want to do because we need to reset
    // the internal time, sync, priority, message numbers, etc.)
    rebuild_client_connection(world)?;
    if let Err(e) = world.resource_mut::<ClientConnection>().connect() {
        error!("Error connecting client: {}", e);
        world.resource_mut::<ClientConnection>().disconnect_reason = Some(e);

        world
            .resource_mut::<NextState<NetworkingState>>()
            .set(NetworkingState::Disconnected);
    }
    Ok(())
}

pub trait ClientCommandsExt {
    /// Start the connection process
    fn connect_client(&mut self);

    /// Disconnect the client
    fn disconnect_client(&mut self);
}

impl ClientCommandsExt for Commands<'_, '_> {
    fn connect_client(&mut self) {
        self.queue(|world: &mut World| world.connect_client());
    }

    fn disconnect_client(&mut self) {
        self.queue(|world: &mut World| world.disconnect_client());
    }
}

impl ClientCommandsExt for World {
    fn connect_client(&mut self) {
        self.resource_mut::<NextState<NetworkingState>>()
            .set(NetworkingState::Connecting);
    }

    fn disconnect_client(&mut self) {
        self.resource_mut::<NextState<NetworkingState>>()
            .set(NetworkingState::Disconnecting);
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use crate::{
        prelude::{client::ClientCommandsExt, server},
        tests::host_server_stepper::HostServerStepper,
    };

    #[derive(Resource, Default)]
    struct CheckCounter(usize);

    fn receive_connect_event(
        mut reader: EventReader<server::ConnectEvent>,
        mut res: ResMut<CheckCounter>,
    ) {
        for event in reader.read() {
            res.0 += 1;
        }
    }

    fn receive_disconnect_event(
        mut reader: EventReader<server::DisconnectEvent>,
        mut res: ResMut<CheckCounter>,
    ) {
        for event in reader.read() {
            res.0 += 1;
        }
    }

    #[test]
    fn test_host_server_connect_event() {
        let mut stepper = HostServerStepper::default_no_init();

        stepper
            .server_app
            .init_resource::<CheckCounter>()
            .add_systems(Update, receive_connect_event);
        stepper.init();
        assert_eq!(stepper.server_app.world().resource::<CheckCounter>().0, 2); // 2 because local client as well as external client connect
    }

    /// Check that a server::Disconnect event is emitted on the server when the local client disconnects
    #[test]
    fn test_host_server_disconnect_event() {
        let mut stepper = HostServerStepper::default();

        stepper
            .server_app
            .init_resource::<CheckCounter>()
            .add_systems(Update, receive_disconnect_event);
        stepper.client_app.world_mut().disconnect_client();
        stepper.frame_step();
        stepper.frame_step();

        stepper.server_app.world_mut().disconnect_client();
        stepper.frame_step();
        stepper.frame_step();
        assert_eq!(stepper.server_app.world().resource::<CheckCounter>().0, 2); // 2 because local client as well as external client disconnect
    }
}
