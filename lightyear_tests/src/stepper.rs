use crate::protocol::ProtocolPlugin;
#[cfg(not(feature = "std"))]
use alloc::vec;
use bevy::MinimalPlugins;
use bevy::app::PluginsState;
use bevy::input::InputPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::time::Duration;
use lightyear::prelude::{client::*, server::*, *};
#[cfg(feature = "test_utils")]
use lightyear_core::test::TestHelper;
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_replication::delta::DeltaManager;

pub(crate) const SERVER_PORT: u16 = 56789;
pub(crate) const SERVER_ADDR: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT));

pub(crate) const STEAM_APP_ID: u32 = 480; // Steamworks App ID for Spacewar, used for testing

pub(crate) const TICK_DURATION: Duration = Duration::from_millis(10);

/// Stepper with:
/// - n client in one 'client' App
/// - 1 server in another App, with n ClientOf connected to each client
///
/// Connected via crossbeam channels, and using Netcode for connection
/// We create two separate apps to make it easy to order the client and server updates.
pub struct ClientServerStepper {
    pub client_apps: Vec<App>,
    pub server_app: App,
    pub client_entities: Vec<Entity>,
    pub server_entity: Entity,
    pub client_of_entities: Vec<Entity>,
    pub host_client_entity: Option<Entity>,
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    pub current_time: bevy::platform::time::Instant,
}

impl ClientServerStepper {
    pub fn single() -> Self {
        Self::with_clients(1)
    }

    pub fn with_clients(n: usize) -> Self {
        let mut stepper = Self::default_no_init();
        for _ in 0..n {
            stepper.new_client();
        }
        stepper.init();
        stepper
    }

    pub fn host_server() -> Self {
        let mut stepper = Self::default_no_init();
        stepper.new_host_client();
        stepper.new_client();
        stepper.init();
        stepper
    }
}

impl ClientServerStepper {
    pub fn new(tick_duration: Duration, frame_duration: Duration) -> Self {
        let mut server_app = App::new();
        // NOTE: we add LogPlugin so that tracing works
        server_app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
        ));
        server_app.add_plugins((server::ServerPlugins { tick_duration }, RoomPlugin));
        // ProtocolPlugin needs to be added AFTER InputPlugin
        server_app.add_plugins(ProtocolPlugin);
        let server_entity = server_app
            .world_mut()
            .spawn((
                NetcodeServer::new(lightyear_netcode::server_plugin::NetcodeConfig::default()),
                DeltaManager::default(),
            ))
            .id();
        Self {
            client_apps: vec![],
            server_app,
            client_entities: vec![],
            server_entity,
            host_client_entity: None,
            client_of_entities: vec![],
            frame_duration,
            tick_duration,
            current_time: bevy::platform::time::Instant::now(),
        }
    }

    /// Add a new host-client: we will add the client on the server app
    pub(crate) fn new_host_client(&mut self) {
        // the server app will contain both client and server plugins
        self.server_app.add_plugins(client::ClientPlugins {
            tick_duration: self.tick_duration,
        });
        self.host_client_entity = Some(
            self.server_app
                .world_mut()
                .spawn((
                    // Client + LinkOf = HostServer
                    Client::default(),
                    LinkOf {
                        server: self.server_entity,
                    },
                    // Note: no need to add ReplicationSender/Receiver on the host-client entity
                    // TODO: maybe don't add Link either?
                    Link::new(None),
                    Linked,
                ))
                .id(),
        );
    }

    pub(crate) fn new_client(&mut self) -> usize {
        let mut client_app = App::new();
        client_app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
        ));
        client_app.add_plugins(client::ClientPlugins {
            tick_duration: self.tick_duration,
        });
        // ProtocolPlugin needs to be added AFTER ClientPlugins, InputPlugin, because we need the PredictionRegistry to exist
        client_app.add_plugins(ProtocolPlugin);
        client_app.finish();
        client_app.cleanup();
        let client_id = self.client_entities.len();
        let (crossbeam_client, crossbeam_server) = lightyear_crossbeam::CrossbeamIo::new_pair();

        let auth = Authentication::Manual {
            server_addr: SERVER_ADDR,
            protocol_id: Default::default(),
            private_key: Default::default(),
            client_id: client_id as u64,
        };
        self.client_entities.push(
            client_app
                .world_mut()
                .spawn((
                    Client::default(),
                    // Send pings every frame, so that the Acks are sent every frame
                    PingManager::new(PingConfig {
                        ping_interval: Duration::default(),
                    }),
                    ReplicationSender::default(),
                    ReplicationReceiver::default(),
                    NetcodeClient::new(auth, NetcodeConfig::default()).unwrap(),
                    crossbeam_client,
                    #[cfg(feature = "test_utils")]
                    TestHelper::default(),
                    PredictionManager::default(),
                ))
                .id(),
        );
        self.client_of_entities.push(
            self.server_app
                .world_mut()
                .spawn((
                    LinkOf {
                        server: self.server_entity,
                    },
                    // Send pings every frame, so that the Acks are sent every frame
                    PingManager::new(PingConfig {
                        ping_interval: Duration::default(),
                    }),
                    // TODO: we want the ReplicationSender/Receiver to be added automatically when ClientOf is created, but with configs pre-specified by the server
                    ReplicationSender::default(),
                    ReplicationReceiver::default(),
                    // we will act like each client has a different port
                    Link::new(None),
                    PeerAddr(SocketAddr::new(
                        core::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                        client_id as u16,
                    )),
                    // For Crossbeam we need to mark the IO as Linked, as there is no ServerLink to do that for us
                    Linked,
                    crossbeam_server,
                    #[cfg(feature = "test_utils")]
                    TestHelper::default(),
                ))
                .id(),
        );
        self.client_apps.push(client_app);
        client_id
    }

    pub(crate) fn new_steam_client(&mut self) -> usize {
        let mut client_app = App::new();
        client_app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
        ));

        // the steam resources need to be added before the ClientPlugins
        client_app.add_steam_resources(STEAM_APP_ID);

        client_app.add_plugins(client::ClientPlugins {
            tick_duration: self.tick_duration,
        });
        // ProtocolPlugin needs to be added AFTER ClientPlugins, InputPlugin, because we need the PredictionRegistry to exist
        client_app.add_plugins(ProtocolPlugin);
        client_app.finish();
        client_app.cleanup();
        let client_id = self.client_entities.len();
        self.client_entities.push(
            client_app
                .world_mut()
                .spawn((
                    Client::default(),
                    // Send pings every frame, so that the Acks are sent every frame
                    PingManager::new(PingConfig {
                        ping_interval: Duration::default(),
                    }),
                    ReplicationSender::default(),
                    ReplicationReceiver::default(),
                    SteamClientIo {
                        target: ConnectTarget::Addr(SERVER_ADDR),
                        config: Default::default(),
                    },
                    #[cfg(feature = "test_utils")]
                    TestHelper::default(),
                    PredictionManager::default(),
                ))
                .id(),
        );
        self.client_apps.push(client_app);
        client_id
    }

    /// Disconnect the last client
    pub(crate) fn disconnect_client(&mut self) {
        let client_entity = self.client_entities.pop().unwrap();
        let server_entity = self.client_of_entities.pop().unwrap();
        let mut client_app = self.client_apps.pop().unwrap();

        client_app
            .world_mut()
            .trigger_targets(Disconnect, client_entity);
        // on the server normally we should wait for the client to send a Disconnect message, but if we despawn the client entity
        // the crossbeam io gets severed
        self.server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(Disconnected { reason: None });
        client_app.world_mut().flush();
        self.server_app.world_mut().flush();
        client_app.world_mut().despawn(client_entity);
        self.server_app.world_mut().despawn(server_entity);
        self.frame_step(1);
    }

    pub(crate) fn default_no_init() -> Self {
        let frame_duration = TICK_DURATION;
        let tick_duration = TICK_DURATION;
        Self::new(tick_duration, frame_duration)
    }

    pub fn client_app(&mut self) -> &mut App {
        assert_eq!(self.client_apps.len(), 1);
        &mut self.client_apps[0]
    }

    pub(crate) fn client_tick(&self, id: usize) -> Tick {
        self.client_apps[id]
            .world()
            .entity(self.client_entities[id])
            .get::<LocalTimeline>()
            .unwrap()
            .tick()
    }
    pub(crate) fn server_tick(&self) -> Tick {
        self.server_app
            .world()
            .entity(self.server_entity)
            .get::<LocalTimeline>()
            .unwrap()
            .tick()
    }

    pub fn host_client(&self) -> EntityRef<'_> {
        self.server_app
            .world()
            .entity(self.host_client_entity.unwrap())
    }

    pub fn host_client_mut(&mut self) -> EntityWorldMut<'_> {
        self.server_app
            .world_mut()
            .entity_mut(self.host_client_entity.unwrap())
    }

    pub fn client(&self, id: usize) -> EntityRef<'_> {
        self.client_apps[id]
            .world()
            .entity(self.client_entities[id])
    }

    pub fn client_mut(&mut self, id: usize) -> EntityWorldMut<'_> {
        self.client_apps[id]
            .world_mut()
            .entity_mut(self.client_entities[id])
    }

    pub fn server(&self) -> EntityRef<'_> {
        self.server_app.world().entity(self.server_entity)
    }

    pub fn server_mut(&mut self) -> EntityWorldMut<'_> {
        self.server_app.world_mut().entity_mut(self.server_entity)
    }

    pub fn client_of(&self, id: usize) -> EntityRef<'_> {
        self.server_app.world().entity(self.client_of_entities[id])
    }

    pub fn client_of_mut(&mut self, id: usize) -> EntityWorldMut<'_> {
        self.server_app
            .world_mut()
            .entity_mut(self.client_of_entities[id])
    }

    pub(crate) fn init(&mut self) {
        if matches!(
            self.server_app.plugins_state(),
            PluginsState::Ready | PluginsState::Adding
        ) {
            self.server_app.finish();
            self.server_app.cleanup();
        }

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::platform::time::Instant::now();
        self.current_time = now;
        self.server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        self.server_app
            .world_mut()
            .trigger_targets(Start, self.server_entity);
        // For HostServer, the server needs to be started before the client,
        // so make sure it is started
        self.server_app.world_mut().flush();
        for i in 0..self.client_entities.len() {
            self.client_apps[i]
                .world_mut()
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            self.client_apps[i]
                .world_mut()
                .trigger_targets(Connect, self.client_entities[i]);
        }
        if let Some(host) = self.host_client_entity {
            self.server_app.world_mut().trigger_targets(Connect, host);
        }

        self.wait_for_connection();
        self.wait_for_sync();
    }

    /// Frame step until all clients are connected
    pub(crate) fn wait_for_connection(&mut self) {
        for _ in 0..50 {
            if (0..self.client_entities.len())
                .all(|client_id| self.client(client_id).contains::<Connected>())
            {
                info!("Clients are all connected");
                break;
            }
            self.frame_step(1);
        }
    }

    // Advance the world until the client is synced
    pub(crate) fn wait_for_sync(&mut self) {
        for _ in 0..50 {
            if (0..self.client_entities.len()).all(|client_id| {
                self.client(client_id).contains::<IsSynced<InputTimeline>>()
                    && self
                        .client(client_id)
                        .contains::<IsSynced<InterpolationTimeline>>()
            }) {
                info!("Clients are all synced");
                break;
            }
            self.frame_step(1);
        }
    }

    pub fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.client_apps.iter_mut().for_each(|client_app| {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        });
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        #[cfg(feature = "test_utils")]
        mock_instant::global::MockClock::advance(duration);
        #[cfg(not(feature = "test_utils"))]
        std::thread::sleep(duration);
    }

    pub(crate) fn flush(&mut self) {
        self.client_apps.iter_mut().for_each(|client_app| {
            client_app.world_mut().flush();
        });
        self.server_app.world_mut().flush();
    }

    /// Advance the world by one frame duration
    pub fn frame_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.frame_duration);
            // we want to log the next frame's tick before the frame starts
            let client_tick = if self.client_entities.is_empty() {
                None
            } else {
                Some(self.client_tick(0) + 1)
            };
            let server_tick = self.server_tick() + 1;
            info!(?client_tick, ?server_tick, "Frame step");
            self.client_apps
                .iter_mut()
                .enumerate()
                .for_each(|(i, client_app)| {
                    error_span!("client", ?i).in_scope(|| client_app.update());
                });
            error_span!("server").in_scope(|| self.server_app.update());
        }
    }

    /// Advance the world by one frame duration
    pub fn frame_step_server_first(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.frame_duration);
            // we want to log the next frame's tick before the frame starts
            let client_tick = if self.client_entities.is_empty() {
                None
            } else {
                Some(self.client_tick(0) + 1)
            };
            let server_tick = self.server_tick() + 1;
            info!(?client_tick, ?server_tick, "Frame step");
            error_span!("server").in_scope(|| self.server_app.update());
            self.client_apps
                .iter_mut()
                .enumerate()
                .for_each(|(i, client_app)| {
                    error_span!("client", ?i).in_scope(|| client_app.update());
                });
        }
    }

    pub(crate) fn tick_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.tick_duration);
            self.client_apps.iter_mut().for_each(|client_app| {
                client_app.update();
            });
            self.server_app.update();
        }
    }
}
