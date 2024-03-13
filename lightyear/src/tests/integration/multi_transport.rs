//! Tests related to the server using multiple transports at the same time to connect to clients
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

use crate::client as crate_client;
use crate::connection::netcode::generate_key;
use crate::connection::server::{NetServer, ServerConnections};
use crate::prelude::client::{
    Authentication, ClientConfig, ClientConnection, InputConfig, InterpolationConfig, NetClient,
    NetConfig, PredictionConfig, SyncConfig,
};
use crate::prelude::server::{NetcodeConfig, ServerConfig};
use crate::prelude::*;
use crate::server as crate_server;
use crate::transport::LOCAL_SOCKET;

use crate::protocol::*;
use crate::shared::replication::components::Replicate;
use crate::tests::protocol::*;
use crate::tests::stepper::{BevyStepper, Step};

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
        let client_id = 0;
        let server_addr = LOCAL_SOCKET;

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();
        let auth = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id,
        };

        // client net config 1: use local channels
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let client_io = IoConfig::from_transport(TransportConfig::LocalChannel {
            recv: from_server_recv,
            send: to_server_send,
        });
        let client_params = (LOCAL_SOCKET, to_server_recv, from_server_send);
        let net_config_1 = NetConfig::Netcode {
            auth: auth.clone(),
            config: client::NetcodeConfig::default(),
            io: client_io,
        };

        // TODO: maybe we don't need the server Channels transport and instead we can just have multiple
        //  concurrent LocalChannel connections? seems easier to reason about!
        let server_io_1 = IoConfig::from_transport(TransportConfig::Channels {
            channels: vec![client_params],
        });

        // client net config 2: use local channels
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let client_io = IoConfig::from_transport(TransportConfig::LocalChannel {
            recv: from_server_recv,
            send: to_server_send,
        });
        let client_params = (LOCAL_SOCKET, to_server_recv, from_server_send);
        let net_config_2 = NetConfig::Netcode {
            auth,
            config: client::NetcodeConfig::default(),
            io: client_io,
        };

        let server_io_2 = IoConfig::from_transport(TransportConfig::Channels {
            channels: vec![client_params],
        });

        // build server with two distinct transports
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
        let plugin_config = server::PluginConfig::new(config, protocol());
        let plugin = server::ServerPlugin::new(plugin_config);
        server_app.add_plugins(plugin);
        // Initialize Real time (needed only for the first TimeSystem run)
        server_app
            .world
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);

        // get the actual socket address for the udp transport
        let server_addr = server_app
            .world
            .resource::<ServerConnections>()
            .servers
            .last()
            .unwrap()
            .io()
            .local_addr();

        let build_client = |net_config: NetConfig| -> App {
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

            let config = ClientConfig {
                shared: shared_config.clone(),
                net: net_config,
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
        let _ = self
            .client_app_1
            .world
            .resource_mut::<ClientConnection>()
            .connect();
        let _ = self
            .client_app_2
            .world
            .resource_mut::<ClientConnection>()
            .connect();

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_app_1
                .world
                .resource::<ClientConnectionManager>()
                .is_synced()
                && self
                    .client_app_2
                    .world
                    .resource::<ClientConnectionManager>()
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
        mock_instant::MockClock::advance(duration);
    }
}

impl Step for MultiBevyStepper {
    /// Advance the world by one frame duration
    fn frame_step(&mut self) {
        self.advance_time(self.frame_duration);
        self.client_app_1.update();
        self.client_app_2.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(Duration::from_millis(1));
        self.server_app.update();
        std::thread::sleep(Duration::from_millis(1));
    }

    fn tick_step(&mut self) {
        self.advance_time(self.tick_duration);
        self.client_app_1.update();
        self.client_app_2.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(Duration::from_millis(1));
        self.server_app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn test_multi_transport() {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(20),
        incoming_jitter: Duration::from_millis(0),
        incoming_loss: 0.0,
    };
    let mut stepper = MultiBevyStepper::new(
        shared_config,
        SyncConfig::default(),
        PredictionConfig::default(),
        InterpolationConfig::default(),
        frame_duration,
    );
    stepper.init();

    stepper.frame_step();
    stepper.frame_step();
    // since the clients are synced, the ClientMetadata entities should be replicated already
    let client_metadata_1 = stepper
        .client_app_1
        .world
        .query::<&ClientMetadata>()
        .get_single(&stepper.client_app_1.world);
    // dbg!(client_metadata_1);

    // // spawn an entity on the server
    // stepper
    //     .server_app
    //     .world
    //     .spawn((Component1(1.0), Replicate::default()));
    // stepper.frame_step();
    // stepper.frame_step();

    // check that the entity got replicated to both clients
    // (even though they share the same client id)
}
