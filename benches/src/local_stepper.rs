//! Helpers to setup a bevy app where I can just step the world easily.
//! Uses crossbeam channels to mock the network
use bevy::core::TaskPoolThreadAssignmentPolicy;
use bevy::ecs::system::RunSystemOnce;
use bevy::log::{error, Level, LogPlugin};
use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::prelude::{
    default, App, Commands, Mut, Plugin, PluginGroup, Real, Resource, TaskPoolOptions,
    TaskPoolPlugin, Time,
};
use bevy::state::app::StatesPlugin;
use bevy::tasks::available_parallelism;
use bevy::time::TimeUpdateStrategy;
use bevy::utils::HashMap;
use bevy::MinimalPlugins;

use lightyear::connection::netcode::generate_key;
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::prelude::{client, server};
use lightyear::transport::LOCAL_SOCKET;

use crate::protocol::*;

pub trait Step {
    /// Advance the time on the server and client by a given duration

    fn advance_time(&mut self, duration: Duration);

    /// Update the server and then the client(s)
    fn update(&mut self) {
        self.server_update();
        self.client_update();
    }

    /// Update the server
    fn server_update(&mut self);

    /// Update the client(s)
    fn client_update(&mut self);

    /// Advance both apps by one frame duration
    fn frame_step(&mut self);

    /// Advance both apps by on fixed timestep duration
    fn tick_step(&mut self);
}

pub struct LocalBevyStepper {
    pub client_apps: HashMap<ClientId, App>,
    pub server_app: App,
    pub frame_duration: Duration,
    /// fixed timestep duration
    pub tick_duration: Duration,
    pub current_time: bevy::utils::Instant,
}

impl Default for LocalBevyStepper {
    fn default() -> Self {
        let frame_duration = Duration::from_secs_f64(1.0 / 60.0);
        let tick_duration = Duration::from_secs_f64(1.0 / 64.0);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..default()
        };
        let mut stepper = LocalBevyStepper::new(
            1,
            shared_config,
            SyncConfig::default(),
            PredictionConfig::default(),
            InterpolationConfig::default(),
            frame_duration,
        );
        stepper.init();
        stepper
    }
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl LocalBevyStepper {
    pub fn new(
        num_clients: usize,
        shared_config: SharedConfig,
        sync_config: SyncConfig,
        prediction_config: PredictionConfig,
        interpolation_config: InterpolationConfig,
        frame_duration: Duration,
    ) -> Self {
        let now = bevy::utils::Instant::now();
        // Local channels transport only works with server socket = LOCAL_SOCKET
        let server_addr = LOCAL_SOCKET;

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();
        let client_id = 111;

        let mut client_params = vec![];
        let mut client_apps = HashMap::new();
        for i in 0..num_clients {
            // Setup io
            let client_id = i as u64;
            let port = 1234 + i;
            let addr = SocketAddr::from_str(&format!("127.0.0.1:{:?}", port)).unwrap();
            // channels to receive a message from/to server
            let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
            let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
            let client_io = client::IoConfig::from_transport(ClientTransport::LocalChannel {
                recv: from_server_recv,
                send: to_server_send,
            });
            client_params.push((addr, to_server_recv, from_server_send));

            // Setup client
            let mut client_app = App::new();
            client_app.add_plugins((
                MinimalPlugins,
                StatesPlugin,
                // LogPlugin::default(),
            ));
            let auth = Authentication::Manual {
                server_addr,
                protocol_id,
                private_key,
                client_id,
            };
            let config = ClientConfig {
                shared: shared_config,
                net: client::NetConfig::Netcode {
                    auth,
                    config: client::NetcodeConfig::default(),
                    io: client_io,
                },
                sync: sync_config,
                prediction: prediction_config,
                interpolation: interpolation_config,
                ..default()
            };
            client_app.add_plugins((ClientPlugins::new(config), ProtocolPlugin));
            // Initialize Real time (needed only for the first TimeSystem run)
            client_app
                .world_mut()
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            client_apps.insert(ClientId::Netcode(client_id), client_app);
        }

        // Setup server
        let server_io = server::IoConfig::from_transport(ServerTransport::Channels {
            channels: client_params,
        });

        let mut server_app = App::new();
        server_app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            // LogPlugin::default(),
        ));
        let config = ServerConfig {
            shared: shared_config,
            net: vec![server::NetConfig::Netcode {
                config: server::NetcodeConfig::default()
                    .with_protocol_id(protocol_id)
                    .with_key(private_key),
                io: server_io,
            }],
            ..default()
        };
        server_app.add_plugins((ServerPlugins::new(config), ProtocolPlugin));

        // Initialize Real time (needed only for the first TimeSystem run)
        server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        Self {
            client_apps,
            server_app,
            frame_duration,
            tick_duration: shared_config.tick.tick_duration,
            current_time: now,
        }
    }

    pub fn default_n_clients(n: usize) -> Self {
        let frame_duration = Duration::from_secs_f64(1.0 / 60.0);
        let tick_duration = Duration::from_secs_f64(1.0 / 64.0);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..default()
        };
        let mut stepper = LocalBevyStepper::new(
            n,
            shared_config,
            SyncConfig::default(),
            PredictionConfig::default(),
            InterpolationConfig::default(),
            frame_duration,
        );
        stepper.init();
        stepper
    }

    pub fn client_resource<R: Resource>(&self, client_id: ClientId) -> &R {
        self.client_apps
            .get(&client_id)
            .unwrap()
            .world()
            .resource::<R>()
    }

    pub fn client_resource_mut<R: Resource>(&mut self, client_id: ClientId) -> Mut<R> {
        self.client_apps
            .get_mut(&client_id)
            .unwrap()
            .world_mut()
            .resource_mut::<R>()
    }

    pub fn init(&mut self) {
        self.server_app.finish();
        self.server_app.cleanup();
        self.server_app
            .world_mut()
            .run_system_once(|mut commands: Commands| commands.start_server());
        self.client_apps.values_mut().for_each(|client_app| {
            client_app.finish();
            client_app.cleanup();
            client_app
                .world_mut()
                .run_system_once(|mut commands: Commands| commands.connect_client());
        });

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self.client_apps.values().all(|c| {
                c.world()
                    .resource::<client::ConnectionManager>()
                    .is_synced()
            }) {
                return;
            }
            self.frame_step();
        }
    }
}

impl Step for LocalBevyStepper {
    fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        for client_app in self.client_apps.values_mut() {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        }
    }

    fn server_update(&mut self) {
        self.server_app.update();
    }

    fn client_update(&mut self) {
        for client_app in self.client_apps.values_mut() {
            client_app.update();
        }
    }

    fn frame_step(&mut self) {
        self.advance_time(self.frame_duration);
        self.update();
    }

    fn tick_step(&mut self) {
        self.advance_time(self.tick_duration);
        self.update();
    }
}
