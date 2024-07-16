//! Stepper to run tests in host-server mode (client and server are in the same app)
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::ecs::system::RunSystemOnce;
use bevy::input::InputPlugin;
use bevy::prelude::{default, App, Commands, Mut, PluginGroup, Real, Time, World};
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::utils::Duration;
use bevy::MinimalPlugins;

use crate::connection::netcode::generate_key;
use crate::prelude::client::{
    Authentication, ClientCommands, ClientConfig, ClientTransport, InterpolationConfig, NetConfig,
    PredictionConfig, SyncConfig,
};
use crate::prelude::server::{NetcodeConfig, ServerCommands, ServerConfig, ServerTransport};
use crate::prelude::*;
use crate::shared::time_manager::WrappedTime;
use crate::tests::protocol::*;
use crate::transport::LOCAL_SOCKET;

pub const LOCAL_CLIENT_ID: u64 = 111;
pub const EXTERNAL_CLIENT_ID: u64 = 112;

/// Helpers to setup a bevy app where I can just step the world easily
pub trait Step {
    /// Advance both apps by one frame duration
    fn frame_step(&mut self);

    /// Advance both apps by on fixed timestep duration
    fn tick_step(&mut self);
}

pub struct HostServerStepper {
    // App for another external client
    pub client_app: App,
    // App for the host-server (client + server)
    pub server_app: App,
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    pub current_time: bevy::utils::Instant,
}

impl Default for HostServerStepper {
    fn default() -> Self {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let client_config = ClientConfig::default();

        let mut stepper = Self::new(shared_config, client_config, frame_duration);
        stepper.init();
        stepper
    }
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl HostServerStepper {
    pub fn new(
        shared_config: SharedConfig,
        mut client_config: ClientConfig,
        frame_duration: Duration,
    ) -> Self {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();

        // Use local channels instead of UDP for testing
        let addr = LOCAL_SOCKET;
        // channels to receive a message from/to server
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let mut client_io = client::IoConfig::from_transport(ClientTransport::LocalChannel {
            send: to_server_send,
            recv: from_server_recv,
        });

        let mut server_io = server::IoConfig::from_transport(ServerTransport::Channels {
            channels: vec![(addr, to_server_recv, from_server_send)],
        });

        let NetConfig::Netcode { io, .. } = client_config.net.clone() else {
            panic!("Only Netcode transport is supported in tests");
        };
        if let Some(conditioner) = io.conditioner {
            server_io = server_io.with_conditioner(conditioner.clone());
            client_io = client_io.with_conditioner(conditioner.clone());
        }

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();

        // Setup server
        let mut server_app = App::new();
        server_app.add_plugins((MinimalPlugins, StatesPlugin));
        let net_config = server::NetConfig::Netcode {
            config: NetcodeConfig::default()
                .with_protocol_id(protocol_id)
                .with_key(private_key),
            io: server_io,
        };
        let mut shared_host_server = shared_config;
        shared_host_server.mode = Mode::HostServer;
        let config = ServerConfig {
            shared: shared_host_server,
            net: vec![net_config],
            ping: PingConfig {
                // send pings every tick, so that the acks are received every frame
                ping_interval: Duration::default(),
                ..default()
            },
            ..default()
        };
        let plugin = server::ServerPlugins::new(config);
        server_app.add_plugins((plugin, ProtocolPlugin));

        // Add the ClientPlugin to the server_app, to make it host-server mode!
        let mut host_server_client_config = client_config.clone();
        host_server_client_config.shared = shared_host_server;
        host_server_client_config.net = NetConfig::Local {
            id: LOCAL_CLIENT_ID,
        };
        host_server_client_config.ping = PingConfig {
            // send pings every tick, so that the acks are received every frame
            ping_interval: Duration::default(),
            ..default()
        };
        server_app.add_plugins(client::ClientPlugins::new(host_server_client_config));

        // Setup external client
        let mut client_app = App::new();
        client_app.add_plugins((MinimalPlugins, StatesPlugin));
        let net_config = NetConfig::Netcode {
            auth: Authentication::Manual {
                server_addr: addr,
                protocol_id,
                private_key,
                client_id: EXTERNAL_CLIENT_ID,
            },
            config: Default::default(),
            io: client_io,
        };

        client_config.shared = shared_config;
        client_config.ping = PingConfig {
            // send pings every tick, so that the acks are received every frame
            ping_interval: Duration::default(),
            ..default()
        };
        client_config.net = net_config;

        let plugin = client::ClientPlugins::new(client_config);
        client_app.add_plugins((plugin, ProtocolPlugin));

        #[cfg(feature = "leafwing")]
        {
            client_app.add_plugins(LeafwingInputPlugin::<LeafwingInput1>::default());
            client_app.add_plugins(LeafwingInputPlugin::<LeafwingInput2>::default());
            server_app.add_plugins(LeafwingInputPlugin::<LeafwingInput1>::default());
            server_app.add_plugins(LeafwingInputPlugin::<LeafwingInput2>::default());
            client_app.add_plugins(InputPlugin);
        }

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::utils::Instant::now();
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
            frame_duration,
            tick_duration: shared_config.tick.tick_duration,
            current_time: now,
        }
    }

    pub(crate) fn interpolation_tick(&mut self) -> Tick {
        self.client_app.world_mut().resource_scope(
            |world: &mut World, manager: Mut<client::ConnectionManager>| {
                manager
                    .sync_manager
                    .interpolation_tick(world.resource::<TickManager>())
            },
        )
    }

    pub(crate) fn set_client_tick(&mut self, tick: Tick) {
        let new_time = WrappedTime::from_duration(self.tick_duration * (tick.0 as u32));

        self.client_app
            .world_mut()
            .resource_mut::<TimeManager>()
            .set_current_time(new_time);
        self.client_app
            .world_mut()
            .resource_mut::<TickManager>()
            .set_tick_to(tick);
    }

    pub(crate) fn set_server_tick(&mut self, tick: Tick) {
        let new_time = WrappedTime::from_duration(self.tick_duration * (tick.0 as u32));

        self.server_app
            .world_mut()
            .resource_mut::<TimeManager>()
            .set_current_time(new_time);
        self.server_app
            .world_mut()
            .resource_mut::<TickManager>()
            .set_tick_to(tick);
    }

    pub(crate) fn client_tick(&self) -> Tick {
        self.client_app.world().resource::<TickManager>().tick()
    }
    pub(crate) fn server_tick(&self) -> Tick {
        self.server_app.world().resource::<TickManager>().tick()
    }
    pub(crate) fn init(&mut self) {
        self.server_app.finish();
        self.server_app.cleanup();
        self.server_app
            .world_mut()
            .run_system_once(|mut commands: Commands| {
                commands.start_server();
                commands.connect_client();
            });
        self.client_app.finish();
        self.client_app.cleanup();
        self.client_app
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.connect_client());

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .is_synced()
            {
                break;
            }
            self.frame_step();
        }
    }

    pub(crate) fn start(&mut self) {
        self.server_app
            .world_mut()
            .run_system_once(|mut commands: Commands| {
                commands.start_server();
                commands.connect_client();
            });
        self.client_app
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.connect_client());

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .is_synced()
            {
                break;
            }
            self.frame_step();
        }
    }

    pub(crate) fn stop(&mut self) {
        self.server_app
            .world_mut()
            .run_system_once(|mut commands: Commands| {
                commands.stop_server();
                commands.disconnect_client();
            });
        self.client_app
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.disconnect_client());

        // Advance the world to let the disconnection process complete
        for _ in 0..100 {
            self.frame_step();
        }
    }

    pub(crate) fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.client_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        mock_instant::MockClock::advance(duration);
    }
}

impl Step for HostServerStepper {
    /// Advance the world by one frame duration
    fn frame_step(&mut self) {
        self.advance_time(self.frame_duration);
        self.client_app.update();
        self.server_app.update();
    }

    fn tick_step(&mut self) {
        self.advance_time(self.tick_duration);
        self.client_app.update();
        self.server_app.update();
    }
}
