use bevy::utils::Duration;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::prelude::{default, App, Mut, PluginGroup, Real, Time};
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear::client as lightyear_client;
use lightyear::connection::netcode::generate_key;
use lightyear::prelude::client::{
    Authentication, ClientConfig, InputConfig, InterpolationConfig, PredictionConfig, SyncConfig,
};
use lightyear::prelude::server::{NetcodeConfig, ServerConfig};
use lightyear::prelude::*;
use lightyear::server as lightyear_server;

use crate::protocol::*;

/// Helpers to setup a bevy app where I can just step the world easily

pub trait Step {
    /// Advance both apps by one frame duration
    fn frame_step(&mut self);

    /// Advance both apps by on fixed timestep duration
    fn tick_step(&mut self);
}

pub struct BevyStepper {
    pub client_app: App,
    pub server_app: App,
    pub frame_duration: Duration,
    /// fixed timestep duration
    pub tick_duration: Duration,
    pub current_time: bevy::utils::Instant,
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl BevyStepper {
    pub fn new(
        shared_config: SharedConfig,
        sync_config: SyncConfig,
        prediction_config: PredictionConfig,
        interpolation_config: InterpolationConfig,
        conditioner: LinkConditionerConfig,
        frame_duration: Duration,
    ) -> Self {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_span_events(FmtSpan::ENTER)
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();

        // Shared config
        let server_addr = SocketAddr::from_str("127.0.0.1:5000").unwrap();
        let protocol_id = 0;
        let private_key = generate_key();
        let client_id = 111;

        // Setup server
        let mut server_app = App::new();
        server_app.add_plugins(MinimalPlugins.build());
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key);

        let config = ServerConfig {
            shared: shared_config.clone(),
            net: server::NetConfig::Netcode {
                config: netcode_config,
                io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
                    .with_conditioner(conditioner.clone()),
            },
            ping: PingConfig::default(),
            packet: Default::default(),
        };
        let plugin_config = server::PluginConfig::new(config, protocol());
        let plugin = server::ServerPlugin::new(plugin_config);
        server_app.add_plugins(plugin);

        // Setup client
        let mut client_app = App::new();
        client_app.add_plugins(MinimalPlugins.build());
        let auth = Authentication::Manual {
            server_addr,
            protocol_id,
            private_key,
            client_id,
        };
        let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
        let io = Io::from_config(
            IoConfig::from_transport(TransportConfig::UdpSocket(addr))
                .with_conditioner(conditioner.clone()),
        );
        let config = ClientConfig {
            shared: shared_config.clone(),
            sync: sync_config,
            prediction: prediction_config,
            interpolation: interpolation_config,
            net: client::NetConfig::Netcode {
                auth,
                io: IoConfig::from_transport(TransportConfig::UdpSocket(addr))
                    .with_conditioner(conditioner.clone()),
                ..default()
            },
            ..default()
        };
        let plugin_config = client::PluginConfig::new(config, protocol());
        let plugin = client::ClientPlugin::new(plugin_config);
        client_app.add_plugins(plugin);

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::utils::Instant::now();
        client_app
            .world
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        server_app
            .world
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

    pub fn client(&self) -> &Client {
        self.client_app.world.resource::<Client>()
    }

    pub fn client_mut(&mut self) -> Mut<Client> {
        self.client_app.world.resource_mut::<Client>()
    }

    fn server(&self) -> &Server {
        self.server_app.world.resource::<Server>()
    }

    pub(crate) fn init(&mut self) {
        self.client_mut().connect();

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self.client().is_synced() {
                break;
            }
            self.frame_step();
        }
    }
}

impl Step for BevyStepper {
    /// Advance the world by one frame duration
    fn frame_step(&mut self) {
        self.current_time += self.frame_duration;
        self.client_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        mock_instant::MockClock::advance(self.frame_duration);
        self.client_app.update();
        // TODO: maybe for testing use a local io via channels?
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(Duration::from_millis(10));
        self.server_app.update();
        std::thread::sleep(Duration::from_millis(10));
    }

    fn tick_step(&mut self) {
        self.current_time += self.tick_duration;
        self.client_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        mock_instant::MockClock::advance(self.tick_duration);
        self.client_app.update();
        self.server_app.update();
    }
}
