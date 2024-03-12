//! Helpers to setup a bevy app where I can just step the world easily.
//! Uses crossbeam channels to mock the network
use bevy::core::TaskPoolThreadAssignmentPolicy;
use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::prelude::{
    default, App, Mut, PluginGroup, Real, Resource, TaskPoolOptions, TaskPoolPlugin, Time,
};
use bevy::tasks::available_parallelism;
use bevy::time::TimeUpdateStrategy;
use bevy::utils::HashMap;
use bevy::MinimalPlugins;

use lightyear::client as lightyear_client;
use lightyear::connection::netcode::generate_key;
use lightyear::prelude::client::{
    Authentication, ClientConfig, ClientConnection, InputConfig, InterpolationConfig, NetClient,
    NetConfig, PredictionConfig, SyncConfig,
};
use lightyear::prelude::server::{NetcodeConfig, ServerConfig};
use lightyear::prelude::*;
use lightyear::server as lightyear_server;
use lightyear::transport::LOCAL_SOCKET;

use crate::protocol::*;

pub trait Step {
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
            let client_id = i as ClientId;
            let port = 1234 + i;
            let addr = SocketAddr::from_str(&*format!("127.0.0.1:{:?}", port)).unwrap();
            // channels to receive a message from/to server
            let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
            let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
            let client_io = IoConfig::from_transport(TransportConfig::LocalChannel {
                recv: from_server_recv,
                send: to_server_send,
            });
            client_params.push((addr, to_server_recv, from_server_send));

            // Setup client
            let mut client_app = App::new();
            client_app.add_plugins(
                MinimalPlugins
                    .set(TaskPoolPlugin {
                        task_pool_options: TaskPoolOptions {
                            compute: TaskPoolThreadAssignmentPolicy {
                                min_threads: available_parallelism(),
                                max_threads: std::usize::MAX,
                                percent: 1.0,
                            },
                            ..default()
                        },
                    })
                    .build(),
            );
            let auth = Authentication::Manual {
                server_addr,
                protocol_id,
                private_key,
                client_id,
            };
            let config = ClientConfig {
                shared: shared_config.clone(),
                net: NetConfig::Netcode {
                    auth,
                    config: client::NetcodeConfig::default(),
                    io: client_io,
                },
                sync: sync_config.clone(),
                prediction: prediction_config,
                interpolation: interpolation_config.clone(),
                ..default()
            };
            let plugin_config = client::PluginConfig::new(config, protocol());
            let plugin = client::ClientPlugin::new(plugin_config);
            client_app.add_plugins(plugin);
            // Initialize Real time (needed only for the first TimeSystem run)
            client_app
                .world
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            client_apps.insert(client_id, client_app);
        }

        // Setup server
        let server_io = IoConfig::from_transport(TransportConfig::Channels {
            channels: client_params,
        });

        let mut server_app = App::new();
        server_app.add_plugins(
            MinimalPlugins
                .set(TaskPoolPlugin {
                    task_pool_options: TaskPoolOptions {
                        compute: TaskPoolThreadAssignmentPolicy {
                            min_threads: available_parallelism(),
                            max_threads: std::usize::MAX,
                            percent: 1.0,
                        },
                        ..default()
                    },
                })
                .build(),
        );
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key);
        let config = ServerConfig {
            shared: shared_config.clone(),
            net: vec![server::NetConfig::Netcode {
                config: netcode_config,
                io: server_io,
            }],
            ..default()
        };
        let plugin_config = server::PluginConfig::new(config, protocol());
        let plugin = server::ServerPlugin::new(plugin_config);
        server_app.add_plugins(plugin);

        // Initialize Real time (needed only for the first TimeSystem run)
        server_app
            .world
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

    pub fn client_resource<R: Resource>(&self, client_id: ClientId) -> &R {
        self.client_apps
            .get(&client_id)
            .unwrap()
            .world
            .resource::<R>()
    }

    pub fn client_resource_mut<R: Resource>(&mut self, client_id: ClientId) -> Mut<R> {
        self.client_apps
            .get_mut(&client_id)
            .unwrap()
            .world
            .resource_mut::<R>()
    }

    pub fn init(&mut self) {
        self.client_apps.values_mut().for_each(|client_app| {
            let _ = client_app
                .world
                .resource_mut::<ClientConnection>()
                .connect();
        });

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_apps
                .values()
                .all(|c| c.world.resource::<ClientConnectionManager>().is_synced())
            {
                return;
            }
            self.frame_step();
        }
    }
}

impl Step for LocalBevyStepper {
    /// Advance the world by one frame duration
    fn frame_step(&mut self) {
        self.current_time += self.frame_duration;
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app.update();
        for client_app in self.client_apps.values_mut() {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
            client_app.update();
        }
    }

    fn tick_step(&mut self) {
        self.current_time += self.tick_duration;
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app.update();
        for client_app in self.client_apps.values_mut() {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
            client_app.update();
        }
    }
}
