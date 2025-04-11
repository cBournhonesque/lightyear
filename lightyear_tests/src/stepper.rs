use crate::protocol::ProtocolPlugin;
#[cfg(not(feature = "std"))]
use alloc::vec;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::time::Duration;
use lightyear_client::plugin::ClientPlugins;
use lightyear_client::Client;
use lightyear_connection::client::{Connect, Connected};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::id::PeerId;
use lightyear_connection::server::Start;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_netcode::auth::Authentication;
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_netcode::{NetcodeClient, NetcodeServer};
use lightyear_replication::prelude::{ReplicationReceiver, ReplicationSender};
use lightyear_server::plugin::ServerPlugins;
use lightyear_server::Server;

pub const TEST_CLIENT_1: u64 = 1;
const PROTOCOL_ID: u64 = 0;
const KEY: [u8; 32] = [0; 32];
const SERVER_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

/// Stepper with:
/// - 1 client in one App
/// - 1 server in another App
/// Connected via crossbeam channels, and using Netcode for connection
/// We create two separate apps to make it easy to order the client and server updates.
pub struct ClientServerStepper {
    pub client_app: App,
    pub server_app: App,
    pub client_entity: Entity,
    pub server_entity: Entity,
    pub client_1: Entity,
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    pub current_time: bevy::platform_support::time::Instant,
}

impl Default for ClientServerStepper {
    fn default() -> Self {
        let mut stepper = Self::default_no_init();
        stepper.init();
        stepper
    }
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl ClientServerStepper {
    pub fn new(
        tick_duration: Duration,
        frame_duration: Duration,
    ) -> Self {
        let (crossbeam_client, crossbeam_server) = lightyear_crossbeam::CrossbeamIo::new_pair();

        let mut client_app = App::new();
        client_app.add_plugins((MinimalPlugins, StatesPlugin));
        client_app.add_plugins(ProtocolPlugin);
        client_app.add_plugins(ClientPlugins {
            tick_duration,
        });

        let auth = Authentication::Manual {
                server_addr: SERVER_ADDR,
                protocol_id: PROTOCOL_ID,
                private_key: KEY,
                client_id: TEST_CLIENT_1,
        };
        let client_entity = client_app.world_mut().spawn((
            Client,
            ReplicationSender::default(),
            ReplicationReceiver::default(),
            NetcodeClient::new(auth, NetcodeConfig::default()).unwrap(),
            crossbeam_client,
        )).id();

        let mut server_app = App::new();
        server_app.add_plugins((MinimalPlugins, StatesPlugin));
        server_app.add_plugins(ProtocolPlugin);
        server_app.add_plugins(ServerPlugins {
            tick_duration
        });

        let server_entity = server_app.world_mut().spawn((
            Server,
            NetcodeServer::new(lightyear_netcode::server_plugin::NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: KEY,
                ..Default::default()
            })
        )).id();
        let client_1 = server_app.world_mut().spawn((
            ClientOf {
                server: server_entity,
                id: PeerId::Entity,
            },
            // TODO: we want the ReplicationSender/Receiver to be added automatically when ClientOf is created, but with configs pre-specified by the server
            ReplicationSender::default(),
            ReplicationReceiver::default(),
            crossbeam_server
        )).id();

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::platform_support::time::Instant::now();
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

        Self {
            client_app,
            server_app,
            client_entity,
            server_entity,
            client_1,
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

    pub(crate) fn client_tick(&self) -> Tick {
        self.client_app.world().entity(self.client_entity).get::<LocalTimeline>().unwrap().tick()
    }
    pub(crate) fn server_tick(&self) -> Tick {
        self.server_app.world().entity(self.server_entity).get::<LocalTimeline>().unwrap().tick()
    }

    pub(crate) fn client(&self) -> EntityRef {
        self.client_app.world().entity(self.client_entity)
    }

    pub(crate) fn client_mut(&mut self) -> EntityWorldMut {
        self.client_app.world_mut().entity_mut(self.client_entity)
    }

    pub(crate) fn server(&self) -> EntityRef {
        self.server_app.world().entity(self.server_entity)
    }

    pub(crate) fn server_mut(&mut self) -> EntityWorldMut {
        self.server_app.world_mut().entity_mut(self.server_entity)
    }

    pub(crate) fn client_1(&self) -> EntityRef {
        self.server_app.world().entity(self.client_1)
    }

    pub(crate) fn client_1_mut(&mut self) -> EntityWorldMut {
        self.server_app.world_mut().entity_mut(self.client_1)
    }

    pub(crate) fn build(&mut self) {
        self.client_app.finish();
        self.client_app.cleanup();
        self.server_app.finish();
        self.server_app.cleanup();
    }
    pub(crate) fn init(&mut self) {
        self.client_app.world_mut().trigger_targets(Connect, self.client_entity);
        self.server_app.world_mut().trigger_targets(Start, self.server_entity);
        self.wait_for_connection();
        self.wait_for_sync();
    }

    // Advance the world until client is connected
    pub(crate) fn wait_for_connection(&mut self) {
        for _ in 0..100 {
            if self.client().contains::<Connected>() {
                break;
            }
            self.frame_step(1);
        }
    }

    // Advance the world until the client is synced
    pub(crate) fn wait_for_sync(&mut self) {
        // for _ in 0..100 {
        //     if self
        //         .client_app
        //         .world()
        //         .resource::<client::ConnectionManager>()
        //         .is_synced()
        //     {
        //         break;
        //     }
        //     self.frame_step();
        // }
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
