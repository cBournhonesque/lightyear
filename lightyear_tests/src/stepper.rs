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
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::prelude::{client::*, server::*, *};
#[cfg(feature = "test_utils")]
use lightyear_core::test::TestHelper;
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_raw_connection::client::RawClient;
use lightyear_raw_connection::server::RawServer;
use lightyear_replication::delta::DeltaManager;

pub const SERVER_PORT: u16 = 56789;
pub const SERVER_ADDR: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT));

pub const STEAM_APP_ID: u32 = 480; // Steamworks App ID for Spacewar, used for testing

pub const TICK_DURATION: Duration = Duration::from_millis(10);

/// Stepper with:
/// - n client in one 'client' App
/// - 1 server in another App, with n ClientOf connected to each client
///
/// Connected via crossbeam channels, and using Netcode for connection if `raw_server` is false
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
    pub avian_mode: AvianReplicationMode,
}

/// Type of client to add
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientType {
    Host,
    Raw,
    Netcode,
    #[cfg(feature = "steam")]
    Steam,
}

/// Type of server to use
pub enum ServerType {
    Raw,
    Netcode,
    #[cfg(feature = "steam")]
    Steam,
}

/// Configuration for ClientServerStepper
pub struct StepperConfig {
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    pub clients: Vec<ClientType>,
    pub server: ServerType,
    pub init: bool,
    pub server_registry: Option<MetricsRegistry>,
    pub client_registry: Option<MetricsRegistry>,
    pub avian_mode: AvianReplicationMode,
}

impl StepperConfig {
    /// Basic scenario: one netcode client.
    pub fn single() -> Self {
        Self {
            frame_duration: TICK_DURATION,
            tick_duration: TICK_DURATION,
            clients: vec![ClientType::Netcode],
            server: ServerType::Netcode,
            init: true,
            server_registry: None,
            client_registry: None,
            avian_mode: AvianReplicationMode::default(),
        }
    }

    /// Host server scenario: one host client and one netcode client.
    pub fn host_server() -> Self {
        Self {
            frame_duration: TICK_DURATION,
            tick_duration: TICK_DURATION,
            clients: vec![ClientType::Host, ClientType::Netcode],
            server: ServerType::Netcode,
            init: true,
            server_registry: None,
            client_registry: None,
            avian_mode: AvianReplicationMode::default(),
        }
    }

    pub fn with_netcode_clients(n: usize) -> Self {
        Self {
            frame_duration: TICK_DURATION,
            tick_duration: TICK_DURATION,
            clients: vec![ClientType::Netcode; n],
            server: ServerType::Netcode,
            init: true,
            server_registry: None,
            client_registry: None,
            avian_mode: AvianReplicationMode::default(),
        }
    }

    pub fn from_link_types(clients: Vec<ClientType>, server: ServerType) -> Self {
        Self {
            frame_duration: TICK_DURATION,
            tick_duration: TICK_DURATION,
            clients,
            server,
            init: true,
            server_registry: None,
            client_registry: None,
            avian_mode: AvianReplicationMode::default(),
        }
    }
}

impl ClientServerStepper {
    pub fn from_config(config: StepperConfig) -> Self {
        let mut stepper = Self::new_server(
            config.tick_duration,
            config.frame_duration,
            config.server,
            config.avian_mode,
            config.server_registry.clone(),
        );
        for client_type in config.clients {
            stepper.new_client(client_type, config.client_registry.clone());
        }
        if config.init {
            stepper.init();
        }
        stepper
    }
}

impl ClientServerStepper {
    pub fn new_server(
        tick_duration: Duration,
        frame_duration: Duration,
        server_type: ServerType,
        avian_mode: AvianReplicationMode,
        metrics_registry: Option<MetricsRegistry>,
    ) -> Self {
        let mut server_app = App::new();
        // NOTE: we add LogPlugin so that tracing works
        server_app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
            MetricsPlugin::new(metrics_registry),
        ));
        #[cfg(feature = "steam")]
        if matches!(server_type, ServerType::Steam) {
            // the steam resources need to be added before the ServerPlugins
            server_app.add_steam_resources(STEAM_APP_ID);
        }
        server_app.add_plugins((server::ServerPlugins { tick_duration }, RoomPlugin));
        // ProtocolPlugin needs to be added AFTER InputPlugin
        server_app.add_plugins(ProtocolPlugin { avian_mode });
        let mut server = server_app.world_mut().spawn((DeltaManager::default(),));

        match server_type {
            ServerType::Raw => {
                server.insert(RawServer);
            }
            ServerType::Netcode => {
                server.insert(NetcodeServer::new(
                    lightyear_netcode::server_plugin::NetcodeConfig::default(),
                ));
            }
            #[cfg(feature = "steam")]
            ServerType::Steam => {
                let server_addr =
                    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SERVER_PORT));
                server.insert(SteamServerIo {
                    target: ListenTarget::Addr(server_addr),
                    config: SessionConfig::default(),
                });
            }
        }
        let server_entity = server.id();
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
            avian_mode,
        }
    }

    pub fn new_client(
        &mut self,
        client_type: ClientType,
        metrics_registry: Option<MetricsRegistry>,
    ) -> usize {
        let mut client_app = App::new();
        client_app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
            MetricsPlugin::new(metrics_registry),
        ));

        #[cfg(feature = "steam")]
        if client_type == ClientType::Steam {
            // the steam resources need to be added before the ClientPlugins
            client_app.add_steam_resources(STEAM_APP_ID);
        }
        client_app.add_plugins(client::ClientPlugins {
            tick_duration: self.tick_duration,
        });
        // ProtocolPlugin needs to be added AFTER ClientPlugins, InputPlugin, because we need the PredictionRegistry to exist
        client_app.add_plugins(ProtocolPlugin {
            avian_mode: self.avian_mode,
        });
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

        if client_type == ClientType::Host {
            // for host client we don't need auth
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
                        // TODO: maybe don't add Link either?
                        Link::new(None),
                        Linked,
                    ))
                    .id(),
            );
            return 0;
        }
        let mut client = client_app.world_mut().spawn((
            Client::default(),
            // Send pings every frame, so that the Acks are sent every frame
            PingManager::new(PingConfig {
                ping_interval: Duration::default(),
            }),
            ReplicationSender::default(),
            ReplicationReceiver::default(),
            crossbeam_client,
            #[cfg(feature = "test_utils")]
            TestHelper::default(),
            PredictionManager::default(),
        ));
        match client_type {
            ClientType::Host => unreachable!(),
            ClientType::Raw => {
                client.insert(RawClient);
            }
            ClientType::Netcode => {
                client.insert(NetcodeClient::new(auth, NetcodeConfig::default()).unwrap());
            }
            #[cfg(feature = "steam")]
            ClientType::Steam => {
                client.insert(SteamClientIo {
                    target: ConnectTarget::Addr(SERVER_ADDR),
                    config: Default::default(),
                });
            }
        }
        self.client_entities.push(client.id());
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

    /// Disconnect the last client
    pub fn disconnect_client(&mut self) {
        let client_entity = self.client_entities.pop().unwrap();
        let server_entity = self.client_of_entities.pop().unwrap();
        let mut client_app = self.client_apps.pop().unwrap();

        client_app.world_mut().trigger(Disconnect {
            entity: client_entity,
        });
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

    pub fn default_no_init(server_type: ServerType) -> Self {
        let frame_duration = TICK_DURATION;
        let tick_duration = TICK_DURATION;
        Self::new_server(
            tick_duration,
            frame_duration,
            server_type,
            AvianReplicationMode::default(),
            None,
        )
    }

    pub fn client_app(&mut self) -> &mut App {
        assert_eq!(self.client_apps.len(), 1);
        &mut self.client_apps[0]
    }

    pub fn client_tick(&self, id: usize) -> Tick {
        self.client_apps[id]
            .world()
            .resource::<LocalTimeline>().tick()
    }
    pub fn server_tick(&self) -> Tick {
        self.server_app
            .world()
            .resource::<LocalTimeline>().tick()
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

    pub fn init(&mut self) {
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
        self.server_app.world_mut().trigger(Start {
            entity: self.server_entity,
        });
        // For HostServer, the server needs to be started before the client,
        // so make sure it is started
        self.server_app.world_mut().flush();
        for i in 0..self.client_entities.len() {
            self.client_apps[i]
                .world_mut()
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            self.client_apps[i].world_mut().trigger(Connect {
                entity: self.client_entities[i],
            });
        }
        if let Some(host) = self.host_client_entity {
            self.server_app
                .world_mut()
                .trigger(Connect { entity: host });
        }

        self.wait_for_connection();
        self.wait_for_sync();
    }

    /// Frame step until all clients are connected
    pub fn wait_for_connection(&mut self) {
        for _ in 0..50 {
            if (0..self.client_entities.len())
                .all(|client_id| self.client(client_id).contains::<Connected>())
            {
                info!("Clients are all connected");
                break;
            }
            self.tick_step(1);
        }
    }

    // Advance the world until the client is synced
    pub fn wait_for_sync(&mut self) {
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
            self.tick_step(1);
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
        #[cfg(all(not(feature = "test_utils"), feature = "std"))]
        std::thread::sleep(duration);
    }

    pub fn flush(&mut self) {
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

    pub fn tick_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.tick_duration);
            // we want to log the next frame's tick before the frame starts
            let client_tick = if self.client_entities.is_empty() {
                None
            } else {
                Some(self.client_tick(0) + 1)
            };
            let server_tick = self.server_tick() + 1;
            info!(?client_tick, ?server_tick, "Tick step");
            self.client_apps
                .iter_mut()
                .enumerate()
                .for_each(|(i, client_app)| {
                    error_span!("client", ?i).in_scope(|| client_app.update());
                });
            error_span!("server").in_scope(|| self.server_app.update());
        }
    }
}
