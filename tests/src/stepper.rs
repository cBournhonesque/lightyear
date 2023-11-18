use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::prelude::{App, PluginGroup, Real, Time};
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_shared::client::{Authentication, Client, ClientConfig, SyncConfig};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::server::{NetcodeConfig, PingConfig, Server, ServerConfig};
use lightyear_shared::{IoConfig, LinkConditionerConfig, SharedConfig, TransportConfig};

use crate::protocol::{protocol, MyProtocol};

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
    pub current_time: std::time::Instant,
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl BevyStepper {
    pub fn new(
        shared_config: SharedConfig,
        conditioner: LinkConditionerConfig,
        frame_duration: Duration,
    ) -> Self {
        tracing_subscriber::FmtSubscriber::builder()
            .with_span_events(FmtSpan::ENTER)
            .with_max_level(tracing::Level::DEBUG)
            .init();

        // Shared config
        let server_addr = SocketAddr::from_str("127.0.0.1:5000").unwrap();
        let protocol_id = 0;
        let private_key = generate_key();
        let client_id = 111;
        let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
        let fixed_timestep = Duration::from_millis(10);

        // Setup server
        let mut server_app = App::new();
        server_app.add_plugins(MinimalPlugins.build());
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key);
        let config = ServerConfig {
            shared: shared_config.clone(),
            netcode: netcode_config,
            io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
                .with_conditioner(conditioner.clone()),
            ping: PingConfig::default(),
        };
        let plugin_config = lightyear_shared::server::PluginConfig::new(config, protocol());
        let plugin = lightyear_shared::server::Plugin::new(plugin_config);
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
        let config = ClientConfig {
            shared: shared_config.clone(),
            netcode: Default::default(),
            io: IoConfig::from_transport(TransportConfig::UdpSocket(addr))
                .with_conditioner(conditioner.clone()),
            ping: lightyear_shared::client::PingConfig {
                sync_num_pings: 10,
                sync_ping_interval_ms: Duration::from_millis(30),
                ping_interval_ms: Default::default(),
                rtt_ms_initial_estimate: Default::default(),
                jitter_ms_initial_estimate: Default::default(),
                rtt_smoothing_factor: 0.0,
            },
            sync: SyncConfig::default(),
        };
        let plugin_config = lightyear_shared::client::PluginConfig::new(config, protocol(), auth);
        let plugin = lightyear_shared::client::Plugin::new(plugin_config);
        client_app.add_plugins(plugin);

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = std::time::Instant::now();
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

    pub fn client(&self) -> &Client<MyProtocol> {
        self.client_app.world.resource::<Client<MyProtocol>>()
    }

    fn server(&self) -> &Server<MyProtocol> {
        self.server_app.world.resource::<Server<MyProtocol>>()
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
        self.server_app.update();
    }

    fn tick_step(&mut self) {
        todo!();
    }
}
