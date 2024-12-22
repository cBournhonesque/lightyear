//! Tests related to the server using multiple transports at the same time to connect to clients
use crate::client::networking::ClientCommands;
use bevy::core::TaskPoolThreadAssignmentPolicy;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::{
    default, App, Commands, PluginGroup, Real, TaskPoolOptions, TaskPoolPlugin, Time,
};
use bevy::state::app::StatesPlugin;
use bevy::tasks::available_parallelism;
use bevy::time::TimeUpdateStrategy;
use bevy::utils::Duration;
use bevy::MinimalPlugins;

use crate::connection::netcode::generate_key;
use crate::connection::server::{NetServer, ServerConnections};
use crate::prelude::client::{
    Authentication, ClientConfig, ClientConnection, ClientTransport, InterpolationConfig,
    NetClient, NetConfig, PredictionConfig, SyncConfig,
};
use crate::prelude::server::{NetcodeConfig, ServerCommands, ServerConfig, ServerTransport};
use crate::prelude::*;
use crate::tests::protocol::*;
use crate::tests::stepper::BevyStepper;
use crate::transport::LOCAL_SOCKET;

pub(crate) const TEST_CLIENT_ID_1: u64 = 1;
pub(crate) const TEST_CLIENT_ID_2: u64 = 2;

pub struct MultiBevyStepper {
    // first client will use local channels
    pub client_app_1: App,
    // second client will use udp
    pub client_app_2: App,
    pub server_app: App,
    pub frame_duration: Duration,
    /// fixed timestep duration
    pub tick_duration: Duration,
    pub current_time: bevy::utils::Instant,
}

impl Default for MultiBevyStepper {
    fn default() -> Self {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default();
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = Self::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            frame_duration,
        );
        stepper.init();
        stepper
    }
}

impl MultiBevyStepper {
    pub fn new(
        shared_config: SharedConfig,
        sync_config: SyncConfig,
        prediction_config: PredictionConfig,
        interpolation_config: InterpolationConfig,
        frame_duration: Duration,
    ) -> Self {
        let now = bevy::utils::Instant::now();

        // both clients will use the same client id
        let server_addr = LOCAL_SOCKET;

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();
        let auth_1 = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id: TEST_CLIENT_ID_1,
        };
        let auth_2 = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id: TEST_CLIENT_ID_2,
        };

        // client net config 1: use local channels
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let client_io = client::IoConfig::from_transport(ClientTransport::LocalChannel {
            recv: from_server_recv,
            send: to_server_send,
        });
        let client_params = (LOCAL_SOCKET, to_server_recv, from_server_send);
        let net_config_1 = NetConfig::Netcode {
            auth: auth_1,
            config: client::NetcodeConfig::default(),
            io: client_io,
        };

        // TODO: maybe we don't need the server Channels transport and instead we can just have multiple
        //  concurrent LocalChannel connections? seems easier to reason about!
        let server_io_1 = server::IoConfig::from_transport(ServerTransport::Channels {
            channels: vec![client_params],
        });

        // client net config 2: use local channels
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let client_io = client::IoConfig::from_transport(ClientTransport::LocalChannel {
            recv: from_server_recv,
            send: to_server_send,
        });
        let client_params = (LOCAL_SOCKET, to_server_recv, from_server_send);
        let net_config_2 = NetConfig::Netcode {
            auth: auth_2,
            config: client::NetcodeConfig::default(),
            io: client_io,
        };

        let server_io_2 = server::IoConfig::from_transport(ServerTransport::Channels {
            channels: vec![client_params],
        });

        // build server with two distinct transports
        let mut server_app = App::new();
        server_app.add_plugins((MinimalPlugins, StatesPlugin));
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key);
        let config = ServerConfig {
            shared: shared_config,
            net: vec![
                server::NetConfig::Netcode {
                    config: netcode_config.clone(),
                    io: server_io_1,
                },
                server::NetConfig::Netcode {
                    config: netcode_config,
                    io: server_io_2,
                },
            ],
            ..default()
        };
        let plugin = server::ServerPlugins::new(config);
        server_app.add_plugins((plugin, ProtocolPlugin));
        // Initialize Real time (needed only for the first TimeSystem run)
        server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);

        let build_client = |net_config: NetConfig| -> App {
            let mut client_app = App::new();
            client_app.add_plugins((MinimalPlugins, StatesPlugin));

            let config = ClientConfig {
                shared: shared_config,
                net: net_config,
                sync: sync_config,
                prediction: prediction_config,
                interpolation: interpolation_config,
                ..default()
            };
            let plugin = client::ClientPlugins::new(config);
            client_app.add_plugins((plugin, ProtocolPlugin));
            // Initialize Real time (needed only for the first TimeSystem run)
            client_app
                .world_mut()
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            client_app
        };

        Self {
            client_app_1: build_client(net_config_1),
            client_app_2: build_client(net_config_2),
            server_app,
            frame_duration,
            tick_duration: shared_config.tick.tick_duration,
            current_time: now,
        }
    }

    pub fn init(&mut self) {
        self.server_app.finish();
        self.server_app.cleanup();
        let _ = self
            .server_app
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.start_server());
        self.client_app_1.finish();
        self.client_app_1.cleanup();
        let _ = self
            .client_app_1
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.connect_client());
        self.client_app_2.finish();
        self.client_app_2.cleanup();
        let _ = self
            .client_app_2
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.connect_client());

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_app_1
                .world()
                .resource::<client::ConnectionManager>()
                .is_synced()
                && self
                    .client_app_2
                    .world()
                    .resource::<client::ConnectionManager>()
                    .is_synced()
            {
                return;
            }
            self.frame_step();
        }
    }

    pub(crate) fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.client_app_1
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.client_app_2
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        mock_instant::global::MockClock::advance(duration);
    }

    pub(crate) fn flush(&mut self) {
        self.client_app_1.world_mut().flush();
        self.client_app_2.world_mut().flush();
        self.server_app.world_mut().flush();
    }

    /// Advance the world by one frame duration
    pub(crate) fn frame_step(&mut self) {
        self.advance_time(self.frame_duration);
        self.client_app_1.update();
        self.client_app_2.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(Duration::from_millis(1));
        self.server_app.update();
        std::thread::sleep(Duration::from_millis(1));
    }

    pub(crate) fn tick_step(&mut self) {
        self.advance_time(self.tick_duration);
        self.client_app_1.update();
        self.client_app_2.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(Duration::from_millis(1));
        self.server_app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}
