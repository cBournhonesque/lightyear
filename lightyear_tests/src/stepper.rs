use crate::protocol::ProtocolPlugin;
#[cfg(not(feature = "std"))]
use alloc::vec;
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::time::Duration;
use lightyear::prelude::Link;
use lightyear::prelude::{client, server, *};
use lightyear_connection::client::{Connect, Connected};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::server::Start;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_netcode::auth::Authentication;
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_netcode::{NetcodeClient, NetcodeServer};
use lightyear_replication::prelude::{NetworkVisibilityPlugin, ReplicationReceiver, ReplicationSender};
use lightyear_sync::prelude::client::{Input, Interpolation, InterpolationTimeline, IsSynced};

const PROTOCOL_ID: u64 = 0;
const KEY: [u8; 32] = [0; 32];
const SERVER_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

/// Stepper with:
/// - n client in one 'client' App
/// - 1 server in another App, with n ClientOf connected to each client
/// Connected via crossbeam channels, and using Netcode for connection
/// We create two separate apps to make it easy to order the client and server updates.
pub struct ClientServerStepper<const N: usize = 1> {
    pub client_app: App,
    pub server_app: App,
    pub client_entities: [Entity; N],
    pub server_entity: Entity,
    pub client_of_entities: [Entity; N],
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    pub current_time: bevy::platform::time::Instant,
}

impl ClientServerStepper<1> {
    pub fn single() -> Self {
        Self::default()
    }
}


// Do not forget to use --features mock_time when using the LinkConditioner
impl<const N: usize> ClientServerStepper<N> {
    pub fn new(
        tick_duration: Duration,
        frame_duration: Duration,
    ) -> Self {
        let mut server_app = App::new();
        server_app.add_plugins((MinimalPlugins, StatesPlugin));
        server_app.add_plugins(ProtocolPlugin);
        server_app.add_plugins(NetworkVisibilityPlugin);
        server_app.add_plugins(server::ServerPlugins {
            tick_duration
        });
        let server_entity = server_app.world_mut().spawn((
            server::Server::default(),
            NetcodeServer::new(lightyear_netcode::server_plugin::NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: KEY,
                ..Default::default()
            })
        )).id();

        let mut client_app = App::new();
        client_app.add_plugins((MinimalPlugins, StatesPlugin));
        client_app.add_plugins(ProtocolPlugin);
        client_app.add_plugins(NetworkVisibilityPlugin);
        client_app.add_plugins(client::ClientPlugins {
            tick_duration,
        });

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::platform::time::Instant::now();
        client_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);

        let mut client_entities = [Entity::PLACEHOLDER; N];
        let mut client_of_entities = [Entity::PLACEHOLDER; N];
        for client_id in 0..N {
            let (crossbeam_client, crossbeam_server) = lightyear_crossbeam::CrossbeamIo::new_pair();

            let auth = Authentication::Manual {
                server_addr: SERVER_ADDR,
                protocol_id: PROTOCOL_ID,
                private_key: KEY,
                client_id: client_id as u64,
            };
            client_entities[client_id] = client_app.world_mut().spawn((
                client::Client::default(),
                // Send pings every frame, so that the Acks are sent every frame
                PingManager::new(PingConfig {
                    ping_interval: Duration::default(),
                    ..default()
                }, tick_duration),
                ReplicationSender::default(),
                ReplicationReceiver::default(),
                NetcodeClient::new(auth, NetcodeConfig::default()).unwrap(),
                crossbeam_client,
                PredictionManager::default(),
            )).id();
            client_of_entities[client_id] = server_app.world_mut().spawn((
                ClientOf {
                    server: server_entity,
                    id: PeerId::Entity,
                },
                // Send pings every frame, so that the Acks are sent every frame
                PingManager::new(PingConfig {
                    ping_interval: Duration::default(),
                    ..default()
                }, tick_duration),
                // TODO: we want the ReplicationSender/Receiver to be added automatically when ClientOf is created, but with configs pre-specified by the server
                ReplicationSender::default(),
                ReplicationReceiver::default(),
                // we will act like each client has a different port
                Link::new(SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), client_id as u16), None),
                crossbeam_server
            )).id();
        }

        Self {
            client_app,
            server_app,
            client_entities,
            server_entity,
            client_of_entities,
            frame_duration,
            tick_duration,
            current_time: now,
        }
    }

    pub(crate) fn default_no_init() -> Self {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let mut stepper = Self::new(tick_duration, frame_duration);
        stepper.build();
        stepper
    }

    pub(crate) fn default() -> Self {
        let mut stepper = Self::default_no_init();
        stepper.init();
        stepper
    }

    pub(crate) fn client_tick(&self, id: usize) -> Tick {
        self.client_app.world().entity(self.client_entities[id]).get::<LocalTimeline>().unwrap().tick()
    }
    pub(crate) fn server_tick(&self) -> Tick {
        self.server_app.world().entity(self.server_entity).get::<LocalTimeline>().unwrap().tick()
    }

    pub(crate) fn client(&self, id: usize) -> EntityRef {
        self.client_app.world().entity(self.client_entities[id])
    }

    pub(crate) fn client_mut(&mut self, id: usize) -> EntityWorldMut {
        self.client_app.world_mut().entity_mut(self.client_entities[id])
    }

    pub(crate) fn server(&self) -> EntityRef {
        self.server_app.world().entity(self.server_entity)
    }

    pub(crate) fn server_mut(&mut self) -> EntityWorldMut {
        self.server_app.world_mut().entity_mut(self.server_entity)
    }

    pub(crate) fn client_of(&self, id: usize) -> EntityRef {
        self.server_app.world().entity(self.client_of_entities[id])
    }

    pub(crate) fn client_of_mut(&mut self, id: usize) -> EntityWorldMut {
        self.server_app.world_mut().entity_mut(self.client_of_entities[id])
    }

    pub(crate) fn build(&mut self) {
        self.client_app.finish();
        self.client_app.cleanup();
        self.server_app.finish();
        self.server_app.cleanup();
    }
    pub(crate) fn init(&mut self) {
        for client_id in 0..N {
            self.client_app.world_mut().trigger_targets(Connect, self.client_entities[client_id]);
        }
        self.server_app.world_mut().trigger_targets(Start, self.server_entity);
        self.wait_for_connection();
        self.wait_for_sync();
    }

    /// Frame step until all clients are connected
    pub(crate) fn wait_for_connection(&mut self) {
        for _ in 0..20 {
            if (0..N).all(|client_id| self.client(client_id).contains::<Connected>()) {
                info!("Clients are all connected");
                break
            }
            self.frame_step(1);
        }
    }

    // Advance the world until the client is synced
    pub(crate) fn wait_for_sync(&mut self) {
        for _ in 0..10 {
            if (0..N).all(|client_id| self.client(client_id).contains::<IsSynced<InputTimeline>>() && self.client(client_id).contains::<IsSynced<InterpolationTimeline>>()) {
                info!("Clients are all synced");
                break
            }
            self.frame_step(1);
        }
    }

    pub(crate) fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.client_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        // mock_instant::global::MockClock::advance(duration);
    }

    pub(crate) fn flush(&mut self) {
        self.client_app.world_mut().flush();
        self.server_app.world_mut().flush();
    }

    /// Advance the world by one frame duration
    pub(crate) fn frame_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.frame_duration);
            // we want to log the next frame's tick before the frame starts
            let client_tick = self.client_tick(0) + 1;
            let server_tick = self.server_tick() + 1;
            info!(?client_tick, ?server_tick, "Frame step");
            self.client_app.update();
            self.server_app.update();
        }
    }

    pub(crate) fn tick_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.tick_duration);
            self.client_app.update();
            self.server_app.update();
        }
    }
}
