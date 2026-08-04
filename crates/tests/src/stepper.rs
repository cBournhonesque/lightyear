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
use lightyear_connection::host::HostClient;
#[cfg(feature = "test_utils")]
use lightyear_core::test::TestHelper;
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_raw_connection::client::RawClient;
use lightyear_raw_connection::server::RawServer;
use lightyear_replication::receive::ReplicationReceiver;
#[cfg(feature = "std")]
use lightyear_udp::server::{ServerUdpIo, ServerUdpPlugin};
#[cfg(feature = "std")]
use lightyear_udp::{UdpIo, UdpPlugin};

pub const SERVER_PORT: u16 = 56789;
pub const SERVER_ADDR: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SERVER_PORT));

pub const STEAM_APP_ID: u32 = 480; // Steamworks App ID for Spacewar, used for testing

pub const TICK_DURATION: Duration = Duration::from_millis(10);

/// Adds the per-peer components needed by the shared test protocol to links created by any IO.
fn configure_server_link(
    trigger: On<Add, LinkOf>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    // A host client also has `LinkOf`, but it is a local Client relationship rather than a remote
    // server peer. Host-specific systems configure it separately.
    if clients.contains(trigger.entity) {
        return;
    }
    commands.entity(trigger.entity).insert((
        PingManager::new(PingConfig {
            ping_interval: Duration::default(),
        }),
        ReplicationSender,
        ReplicationReceiver,
        #[cfg(feature = "test_utils")]
        TestHelper::default(),
    ));
}

#[cfg(feature = "std")]
fn unused_local_udp_addr() -> SocketAddr {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.local_addr().unwrap()
}

/// Returns whether the global input timeline resource carries its synchronization marker.
pub fn input_timeline_is_synced(world: &World) -> bool {
    let Some(component_id) = world.component_id::<InputTimeline>() else {
        return false;
    };
    let Some(entity) = world.resource_entities().get(component_id) else {
        return false;
    };
    world.get::<IsSynced<InputTimeline>>(entity).is_some()
}

/// Stepper with:
/// - n client in one 'client' App
/// - 1 server in another App, with n ClientOf connected to each client
///
/// The IO backend and connection layer are configured independently: for example, the same
/// Netcode scenario can run over deterministic in-process Crossbeam channels, real loopback UDP
/// sockets, or a local WebTransport session. We create separate apps to make client/server update
/// ordering explicit.
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
    pub io: IoType,
    pub server_addr: SocketAddr,
    server_started: bool,
}

/// IO backend used between the stepper's client and server applications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IoType {
    /// In-process channels. This is the default because it is deterministic and supports `no_std`.
    #[default]
    Crossbeam,
    /// Real non-blocking loopback UDP sockets.
    #[cfg(feature = "std")]
    Udp,
    /// A real local WebTransport session using a self-signed certificate.
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    WebTransport,
}

impl IoType {
    /// Whether the server must bind before remote clients can be configured.
    fn requires_bound_server_addr(self) -> bool {
        match self {
            Self::Crossbeam => false,
            #[cfg(feature = "std")]
            Self::Udp => false,
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            Self::WebTransport => true,
        }
    }

    /// Whether progress depends on giving an asynchronous IO task real wall-clock time.
    fn uses_async_io(self) -> bool {
        match self {
            Self::Crossbeam => false,
            #[cfg(feature = "std")]
            Self::Udp => false,
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            Self::WebTransport => true,
        }
    }
}

/// Connection layer used by a stepper client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientType {
    Host,
    Raw,
    Netcode,
    #[cfg(feature = "steam")]
    Steam,
}

/// Connection layer used by the stepper server.
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
    /// IO backend, independent from the raw/netcode connection layer.
    pub io: IoType,
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
            io: IoType::Crossbeam,
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
            io: IoType::Crossbeam,
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
            io: IoType::Crossbeam,
        }
    }

    /// Configures client and server connection layers, leaving IO at its default.
    pub fn from_connection_types(clients: Vec<ClientType>, server: ServerType) -> Self {
        Self {
            frame_duration: TICK_DURATION,
            tick_duration: TICK_DURATION,
            clients,
            server,
            init: true,
            server_registry: None,
            client_registry: None,
            avian_mode: AvianReplicationMode::default(),
            io: IoType::Crossbeam,
        }
    }

    /// Backwards-compatible alias for [`Self::from_connection_types`].
    #[deprecated(note = "use `from_connection_types`; these select connection layers, not IO")]
    pub fn from_link_types(clients: Vec<ClientType>, server: ServerType) -> Self {
        Self::from_connection_types(clients, server)
    }

    /// Selects the IO backend without changing the configured connection layer.
    pub fn with_io(mut self, io: IoType) -> Self {
        self.io = io;
        self
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
            config.io,
        );

        if stepper.io.requires_bound_server_addr() {
            assert!(
                config.init,
                "WebTransport steppers must be initialized so the port-zero server can bind before clients are configured"
            );

            // Host clients add client plugins to the server app, so install those before the app
            // is finalized and started. Remote clients need the actual port selected by the OS.
            for client_type in config
                .clients
                .iter()
                .copied()
                .filter(|client_type| *client_type == ClientType::Host)
            {
                stepper.new_client(client_type, config.client_registry.clone());
            }
            stepper.initialize_server();
            for client_type in config
                .clients
                .into_iter()
                .filter(|client_type| *client_type != ClientType::Host)
            {
                stepper.new_client(client_type, config.client_registry.clone());
            }
        } else {
            for client_type in config.clients {
                stepper.new_client(client_type, config.client_registry.clone());
            }
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
        io: IoType,
    ) -> Self {
        let server_addr = match io {
            IoType::Crossbeam => SERVER_ADDR,
            #[cfg(feature = "std")]
            IoType::Udp => unused_local_udp_addr(),
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            IoType::WebTransport => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0),
        };
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
        #[cfg(feature = "std")]
        if io == IoType::Udp {
            server_app.add_plugins(ServerUdpPlugin);
        }
        server_app.add_observer(configure_server_link);
        // ProtocolPlugin needs to be added AFTER InputPlugin
        server_app.add_plugins(ProtocolPlugin { avian_mode });
        let mut server = server_app.world_mut().spawn_empty();

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
        #[cfg(feature = "std")]
        if io == IoType::Udp {
            server.insert((LocalAddr(server_addr), ServerUdpIo::default()));
        }
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        if io == IoType::WebTransport {
            server.insert((
                LocalAddr(server_addr),
                WebTransportServerIo {
                    certificate: Identity::self_signed(["localhost", "127.0.0.1", "::1"])
                        .expect("test WebTransport identity should be valid"),
                },
            ));
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
            io,
            server_addr,
            server_started: false,
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
        #[cfg(feature = "std")]
        if self.io == IoType::Udp {
            client_app.add_plugins(UdpPlugin);
        }
        // ProtocolPlugin needs to be added AFTER ClientPlugins, InputPlugin, because we need the PredictionRegistry to exist
        client_app.add_plugins(ProtocolPlugin {
            avian_mode: self.avian_mode,
        });
        client_app.finish();
        client_app.cleanup();
        let client_id = self.client_entities.len();

        let auth = Authentication::Manual {
            server_addr: self.server_addr,
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
                        Client,
                        LinkOf {
                            server: self.server_entity,
                        },
                        // TODO: maybe don't add Link either?
                        Link::default(),
                        Linked,
                    ))
                    .id(),
            );
            return 0;
        }
        let mut client = client_app.world_mut().spawn((
            Client,
            // Send pings every frame, so that the Acks are sent every frame
            PingManager::new(PingConfig {
                ping_interval: Duration::default(),
            }),
            ReplicationSender,
            ReplicationReceiver,
            #[cfg(feature = "test_utils")]
            TestHelper::default(),
            PredictionManager::default(),
        ));
        match self.io {
            IoType::Crossbeam => {
                let (crossbeam_client, crossbeam_server) =
                    lightyear_crossbeam::CrossbeamIo::new_pair();
                client.insert(crossbeam_client);
                self.client_of_entities.push(
                    self.server_app
                        .world_mut()
                        .spawn((
                            LinkOf {
                                server: self.server_entity,
                            },
                            Link::default(),
                            PeerAddr(SocketAddr::new(
                                core::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                                client_id as u16,
                            )),
                            // Crossbeam has no listening server that creates this link.
                            Linked,
                            crossbeam_server,
                        ))
                        .id(),
                );
            }
            #[cfg(feature = "std")]
            IoType::Udp => {
                client.insert((
                    LocalAddr(SocketAddr::new(
                        core::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                        0,
                    )),
                    PeerAddr(self.server_addr),
                    UdpIo::default(),
                ));
                // ServerUdpIo creates the corresponding LinkOf after the first datagram arrives.
                self.client_of_entities.push(Entity::PLACEHOLDER);
            }
            #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
            IoType::WebTransport => {
                client.insert((
                    PeerAddr(self.server_addr),
                    WebTransportClientIo {
                        // This native-only test mode enables Lightyear's explicitly dangerous
                        // no-certificate-validation feature for the ephemeral self-signed identity.
                        certificate_digest: String::new(),
                        target: None,
                    },
                ));
                // The listening WebTransport server creates the corresponding LinkOf after the
                // session is accepted.
                self.client_of_entities.push(Entity::PLACEHOLDER);
            }
        }
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
            .insert(Disconnected {
                reason: DisconnectedReason::Unknown,
            });
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
            IoType::Crossbeam,
        )
    }

    pub fn client_app(&mut self) -> &mut App {
        assert_eq!(self.client_apps.len(), 1);
        &mut self.client_apps[0]
    }

    pub fn client_tick(&self, id: usize) -> Tick {
        self.client_apps[id]
            .world()
            .resource::<LocalTimeline>()
            .tick()
    }
    pub fn server_tick(&self) -> Tick {
        self.server_app.world().resource::<LocalTimeline>().tick()
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
        assert_ne!(self.client_of_entities[id], Entity::PLACEHOLDER);
        self.server_app.world().entity(self.client_of_entities[id])
    }

    pub fn client_of_mut(&mut self, id: usize) -> EntityWorldMut<'_> {
        assert_ne!(self.client_of_entities[id], Entity::PLACEHOLDER);
        self.server_app
            .world_mut()
            .entity_mut(self.client_of_entities[id])
    }

    pub fn init(&mut self) {
        self.initialize_server();

        let now = self.current_time;
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

    /// Finalizes and starts the server once.
    ///
    /// WebTransport binds to port zero, so it is started before remote clients are constructed.
    /// Once Aeronet publishes the selected local address, that address is used by both the IO
    /// component and Netcode authentication on each client.
    fn initialize_server(&mut self) {
        if self.server_started {
            return;
        }

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
        self.server_started = true;

        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        if self.io == IoType::WebTransport {
            self.wait_for_webtransport_server_addr();
        }
    }

    /// Waits for the port-zero WebTransport listener to report the address selected by the OS.
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    fn wait_for_webtransport_server_addr(&mut self) {
        for _ in 0..400 {
            if let Some(addr) = self
                .server()
                .get::<LocalAddr>()
                .map(|local_addr| local_addr.0)
                .filter(|addr| addr.port() != 0)
            {
                self.server_addr = addr;
                return;
            }
            self.tick_step(1);
            self.wait_for_async_io();
        }
        panic!("WebTransport server did not bind within two seconds");
    }

    /// Frame step until all clients are connected
    pub fn wait_for_connection(&mut self) {
        let attempts = if self.io.uses_async_io() { 400 } else { 50 };
        for _ in 0..attempts {
            self.refresh_client_of_entities();
            if (0..self.client_entities.len())
                .all(|client_id| self.client(client_id).contains::<Connected>())
                && self
                    .client_of_entities
                    .iter()
                    .all(|entity| *entity != Entity::PLACEHOLDER)
            {
                info!("Clients are all connected");
                break;
            }
            self.tick_step(1);
            self.wait_for_async_io();
        }
    }

    /// Resolves server-side links that listening IO backends create after receiving a packet.
    fn refresh_client_of_entities(&mut self) {
        if self
            .client_of_entities
            .iter()
            .all(|entity| *entity != Entity::PLACEHOLDER)
        {
            return;
        }

        let server_entity = self.server_entity;
        let links = {
            let world = self.server_app.world_mut();
            let mut query = world.query_filtered::<
                (Entity, &LinkOf, Option<&RemoteId>),
                (With<Connected>, Without<HostClient>),
            >();
            query
                .iter(world)
                .filter(|(_, link_of, _)| link_of.server == server_entity)
                .map(|(entity, _, remote_id)| (entity, remote_id.map(|id| id.0)))
                .collect::<Vec<_>>()
        };

        for (entity, remote_id) in &links {
            if let Some(PeerId::Netcode(client_id)) = remote_id
                && let Some(slot) = self.client_of_entities.get_mut((*client_id) as usize)
            {
                *slot = *entity;
            }
        }

        // Raw connections do not carry the stepper's numeric client id. Fill any remaining slots
        // deterministically; this is exact for the common one-client listening-IO case.
        for (entity, _) in links {
            if self.client_of_entities.contains(&entity) {
                continue;
            }
            if let Some(slot) = self
                .client_of_entities
                .iter_mut()
                .find(|slot| **slot == Entity::PLACEHOLDER)
            {
                *slot = entity;
            }
        }
    }

    // Advance the world until the client is synced
    pub fn wait_for_sync(&mut self) {
        let attempts = if self.io.uses_async_io() { 400 } else { 50 };
        for _ in 0..attempts {
            if (0..self.client_entities.len()).all(|client_id| {
                input_timeline_is_synced(self.client_apps[client_id].world())
                    && self
                        .client(client_id)
                        .contains::<IsSynced<InterpolationTimeline>>()
            }) {
                info!("Clients are all synced");
                break;
            }
            self.tick_step(1);
            self.wait_for_async_io();
        }
    }

    /// Lets background IO tasks progress when simulated test time advances without sleeping.
    fn wait_for_async_io(&self) {
        #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
        if self.io == IoType::WebTransport {
            std::thread::sleep(Duration::from_millis(5));
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
            self.refresh_client_of_entities();
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
            self.refresh_client_of_entities();
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
            self.refresh_client_of_entities();
        }
    }
}
