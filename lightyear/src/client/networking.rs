//! Defines the plugin related to the client networking (sending and receiving packets).
use anyhow::{Context, Result};
use std::ops::DerefMut;

use bevy::ecs::system::{RunSystemOnce, SystemChangeTick, SystemParam, SystemState};
use bevy::prelude::ResMut;
use bevy::prelude::*;
use tracing::{error, trace};

use crate::_reexport::{ClientMarker, ReplicationSend};
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::{ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent};
use crate::client::sync::SyncSet;
use crate::connection::client::{ClientConnection, NetClient, NetConfig};
use crate::prelude::{SharedConfig, TickManager, TimeManager};
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::shared::config::Mode;
use crate::shared::events::connection::{IterEntityDespawnEvent, IterEntitySpawnEvent};
use crate::shared::sets::InternalMainSet;
use crate::shared::tick_manager::TickEvent;
use crate::shared::time_manager::is_client_ready_to_send;
use crate::transport::io::IoState;

pub(crate) struct ClientNetworkingPlugin<P: Protocol> {
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for ClientNetworkingPlugin<P> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for ClientNetworkingPlugin<P> {
    fn build(&self, app: &mut App) {
        app
            // STATE
            .init_state::<NetworkingState>()
            // SYSTEM SETS
            .configure_sets(
                PreUpdate,
                InternalMainSet::<ClientMarker>::Receive.run_if(
                    not(SharedConfig::is_host_server_condition).and_then(not(is_disconnected)),
                ),
            )
            .configure_sets(
                PostUpdate,
                // run sync before send because some send systems need to know if the client is synced
                // we don't send packets every frame, but on a timer instead
                (
                    SyncSet.run_if(in_state(NetworkingState::Connected)),
                    InternalMainSet::<ClientMarker>::Send
                        .run_if(is_client_ready_to_send.and_then(not(is_disconnected))),
                )
                    .run_if(not(SharedConfig::is_host_server_condition))
                    .chain(),
            )
            // SYSTEMS
            .add_systems(
                PreUpdate,
                receive::<P>.in_set(InternalMainSet::<ClientMarker>::Receive),
            )
            .add_systems(
                PostUpdate,
                (
                    send::<P>.in_set(InternalMainSet::<ClientMarker>::SendPackets),
                    // TODO: update virtual time with Time<Real> so we have more accurate time at Send time.
                    sync_update::<P>.in_set(SyncSet),
                ),
            );

        // STARTUP
        // TODO: update all systems that need these to only run when needed, so that we don't have to create
        //  a ConnectionManager or a NetConfig at startup
        // Create a new `ClientConnection` and `ConnectionManager` at startup, so that systems
        // that depend on these resources do not panic
        app.world.run_system_once(rebuild_net_config::<P>);

        // CONNECTING
        // Everytime we try to connect, we rebuild the net config because:
        // - we do not call update() while the client is disconnected, so the internal connection's time is wrong
        // - this allows us to take into account any changes to the client config (when building a
        // new client connection and connection manager, which want to do because we need to reset
        // the internal time, sync, priority, message numbers, etc.)
        app.add_systems(
            OnEnter(NetworkingState::Connecting),
            (rebuild_net_config::<P>, connect).run_if(is_disconnected),
        );
        app.add_systems(
            PreUpdate,
            handle_connection_failure.run_if(in_state(NetworkingState::Connecting)),
        );

        // CONNECTED
        app.add_systems(OnEnter(NetworkingState::Connected), on_connect);

        // DISCONNECTED
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

pub(crate) fn receive<P: Protocol>(world: &mut World) {
    trace!("Receive server packets");
    // TODO: here we can control time elapsed from the client's perspective?

    // TODO: THE CLIENT COULD DO PHYSICS UPDATES INSIDE FIXED-UPDATE SYSTEMS
    //  WE SHOULD BE CALLING UPDATE INSIDE THOSE AS WELL SO THAT WE CAN SEND UPDATES
    //  IN THE MIDDLE OF THE FIXED UPDATE LOOPS
    //  WE JUST KEEP AN INTERNAL TIMER TO KNOW IF WE REACHED OUR TICK AND SHOULD RECEIVE/SEND OUT PACKETS?
    //  FIXED-UPDATE.expend() updates the clock zR the fixed update interval
    //  THE NETWORK TICK INTERVAL COULD BE IN BETWEEN FIXED UPDATE INTERVALS
    world.resource_scope(
        |world: &mut World, mut connection: Mut<ConnectionManager<P>>| {
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
                                                        let _ = netclient
                                                            .try_update(delta.as_secs_f64())
                                                            .map_err(|e| {
                                                                error!("Error updating netcode: {}", e);
                                                            });

                                                        if netclient.state() == NetworkingState::Connected {
                                                            // we just connected, do a state transition
                                                            if state.get() != &NetworkingState::Connected {
                                                                next_state.set(NetworkingState::Connected);
                                                            }

                                                            // update the connection (message manager, ping manager, etc.)
                                                            connection.update(
                                                                time_manager.as_ref(),
                                                                tick_manager.as_ref(),
                                                            );
                                                        }

                                                        // RECV PACKETS: buffer packets into message managers
                                                        while let Some(packet) = netclient.recv() {
                                                            connection
                                                                .recv_packet(packet, tick_manager.as_ref())
                                                                .unwrap();
                                                        }
                                                        // RECEIVE: receive packets from message managers
                                                        let mut events = connection.receive(
                                                            world,
                                                            time_manager.as_ref(),
                                                            tick_manager.as_ref(),
                                                        );
                                                        // TODO: run these in EventsPlugin!
                                                        // HANDLE EVENTS
                                                        if !events.is_empty() {
                                                            // Message Events
                                                            P::Message::push_message_events(world, &mut events);

                                                            // SpawnEntity event
                                                            if events.has_entity_spawn() {
                                                                let mut entity_spawn_event_writer = world
                                                                    .get_resource_mut::<Events<EntitySpawnEvent>>()
                                                                    .unwrap();
                                                                for (entity, _) in events.into_iter_entity_spawn() {
                                                                    entity_spawn_event_writer
                                                                        .send(EntitySpawnEvent::new(entity, ()));
                                                                }
                                                            }
                                                            // DespawnEntity event
                                                            if events.has_entity_despawn() {
                                                                let mut entity_despawn_event_writer = world
                                                                    .get_resource_mut::<Events<EntityDespawnEvent>>()
                                                                    .unwrap();
                                                                for (entity, _) in events.into_iter_entity_despawn()
                                                                {
                                                                    entity_despawn_event_writer
                                                                        .send(EntityDespawnEvent::new(entity, ()));
                                                                }
                                                            }

                                                            // Update component events (updates, inserts, removes)
                                                            P::Components::push_component_events(
                                                                world,
                                                                &mut events,
                                                            );
                                                        }
                                                    });
                                            });
                                        });
                                    },
                                )
                            }
                    );
                }
            );
    trace!("finished recv");
}

pub(crate) fn send<P: Protocol>(
    mut netcode: ResMut<ClientConnection>,
    system_change_tick: SystemChangeTick,
    tick_manager: Res<TickManager>,
    time_manager: Res<TimeManager>,
    mut connection: ResMut<ConnectionManager<P>>,
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
pub(crate) fn sync_update<P: Protocol>(
    config: Res<ClientConfig>,
    netclient: Res<ClientConnection>,
    connection: ResMut<ConnectionManager<P>>,
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

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkingState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

fn handle_connection_failure(
    mut next_state: ResMut<NextState<NetworkingState>>,
    mut netclient: ResMut<ClientConnection>,
) {
    // first check the status of the io
    if netclient.io_mut().is_some_and(|io| match &mut io.state {
        IoState::Connecting {
            ref mut error_channel,
        } => match error_channel.try_recv() {
            Ok(Some(e)) => {
                error!("Error starting the io: {}", e);
                io.state = IoState::Disconnected;
                true
            }
            Ok(None) => {
                info!("Io is connected!");
                io.state = IoState::Connected;
                false
            }
            Err(_) => true,
        },
        _ => {
            info!("Io state is not Connecting");
            false
        }
    }) {
        info!("Setting the next state to disconnected because of io");
        next_state.set(NetworkingState::Disconnected);
    }
    if netclient.state() == NetworkingState::Disconnected {
        info!("Setting the next state to disconnected because of client connection error");
        next_state.set(NetworkingState::Disconnected);
    }
}

/// System that runs when we enter the Connected state
/// Updates the ConnectEvent events
fn on_connect(
    mut connect_event_writer: EventWriter<ConnectEvent>,
    netcode: Res<ClientConnection>,
    config: Res<ClientConfig>,
    mut server_connect_event_writer: Option<ResMut<Events<crate::server::events::ConnectEvent>>>,
) {
    connect_event_writer.send(ConnectEvent::new(netcode.id()));

    // in host-server mode, we also want to send a connect event to the server
    if config.shared.mode == Mode::HostServer {
        info!("send connect event to server");
        server_connect_event_writer
            .as_mut()
            .unwrap()
            .send(crate::server::events::ConnectEvent::new(netcode.id()));
    }
}
/// System that runs when we enter the Disconnected state
/// Updates the DisconnectEvent events
fn on_disconnect(
    mut disconnect_event_writer: EventWriter<DisconnectEvent>,
    netcode: Res<ClientConnection>,
    config: Res<ClientConfig>,
    mut server_disconnect_event_writer: Option<
        ResMut<Events<crate::server::events::DisconnectEvent>>,
    >,
) {
    disconnect_event_writer.send(DisconnectEvent::new(()));

    // in host-server mode, we also want to send a connect event to the server
    if config.shared.mode == Mode::HostServer {
        server_disconnect_event_writer
            .as_mut()
            .unwrap()
            .send(crate::server::events::DisconnectEvent::new(netcode.id()));
    }
}

/// This run condition is provided to check if the client is connected.
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>` to avoid having a frame of delay
/// since the `StateTransition` schedule runs after `PreUpdate`
pub(crate) fn is_connected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.map_or(false, |c| c.state() == NetworkingState::Connected)
}

/// This run condition is provided to check if the client is disconnected.
///
/// We check the status of the ClientConnection directly instead of using the `State<NetworkingState>` to avoid having a frame of delay
/// since the `StateTransition` schedule runs after `PreUpdate`
pub(crate) fn is_disconnected(netclient: Option<Res<ClientConnection>>) -> bool {
    netclient.map_or(true, |c| c.state() == NetworkingState::Disconnected)
}

/// This runs only when we enter the [`Connecting`](NetworkingState::Connecting) state.
///
/// We rebuild the [`ClientConnection`] by using the latest [`ClientConfig`].
/// This has several benefits:
/// - the client connection's internal time is up-to-date (otherwise it might not be, since we don't call `update` while disconnected)
/// - we can take into account any changes to the client config
fn rebuild_net_config<P: Protocol>(world: &mut World) {
    let client_config = world.resource::<ClientConfig>().clone();
    if client_config.shared.mode == Mode::HostServer {
        assert!(
            matches!(client_config.net, NetConfig::Local { .. }),
            "When running in HostServer mode, the client connection needs to be of type Local"
        );
    }

    // insert a new connection manager (to reset sync, priority, message numbers, etc.)
    let connection_manager = ConnectionManager::<P>::new(
        world.resource::<P>().channel_registry(),
        client_config.packet.clone(),
        client_config.sync.clone(),
        client_config.ping.clone(),
        client_config.prediction.input_delay_ticks,
    );
    world.insert_resource(connection_manager);

    // drop the previous client connection to make sure we release any resources before creating the new one
    world.remove_resource::<ClientConnection>();
    // insert the new client connection
    let netclient = client_config.net.clone().build_client();
    world.insert_resource(netclient);
}

/// Connect the client
fn connect(mut netclient: ResMut<ClientConnection>) {
    info!("calling connect on netclient");
    let _ = netclient
        .connect()
        .inspect_err(|e| error!("Error connecting: {e:?}"));
}

/// This system param is used to connect/disconnect the client.
#[derive(SystemParam)]
pub struct ClientConnectionParam<'w, 's> {
    next_state: ResMut<'w, NextState<NetworkingState>>,
    connection: ResMut<'w, ClientConnection>,
    config: Res<'w, ClientConfig>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's> ClientConnectionParam<'w, 's> {
    /// Public system that should be used by the user to connect
    pub fn connect(&mut self) -> Result<()> {
        // self.connection.connect().context("Error connecting")?;
        let next_state = match self.config.shared.mode {
            // in host server mode, there is no connecting phase, we directly become connected
            // (because the networking systems don't run so we cannot go through the Connecting state)
            Mode::HostServer => NetworkingState::Connected,
            _ => NetworkingState::Connecting,
        };
        self.next_state.set(next_state);
        Ok(())
    }

    /// Public system that should be used by the user to disconnect
    pub fn disconnect(&mut self) -> Result<()> {
        self.connection
            .disconnect()
            .context("Error disconnecting")?;
        self.next_state.set(NetworkingState::Disconnected);
        Ok(())
    }
}
