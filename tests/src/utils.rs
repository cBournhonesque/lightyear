use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::prelude::{App, Fixed, PluginGroup, Real, Time, Virtual};
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_shared::client::{Authentication, ClientConfig, SyncConfig};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::server::{NetcodeConfig, PingConfig, ServerConfig};
use lightyear_shared::{
    IoConfig, LinkConditionerConfig, SharedConfig, TickConfig, TransportConfig,
};

use crate::protocol::protocol;

pub fn tick(app: &mut App) {
    let fxt = app.world.resource_mut::<Time<Fixed>>();
    let timestep = fxt.timestep();
    let time = app.world.resource_mut::<Time<Virtual>>();
    // time.advance_by(timestep);
    app.update();
}

// pub fn client(app: &mut App) -> &Client<MyProtocol> {
//     app.world.resource::<Client<MyProtocol>>()
// }
//
// pub fn server(app: &mut App) -> &Server<MyProtocol> {
//     app.world.resource::<Server<MyProtocol>>()
// }

#[macro_export]
macro_rules! tick_once {
    () => {
        mock_instant::MockClock::advance(frame_duration);
        tick(&mut client_app);
        tick(&mut server_app);
    };
}

pub fn init_bevy_step() -> (App, App) {
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
    let conditioner = LinkConditionerConfig {
        incoming_latency: 45,
        incoming_jitter: 3,
        incoming_loss: 0.0,
    };
    let shared_config = SharedConfig {
        enable_replication: false,
        tick: TickConfig::new(fixed_timestep),
        ..Default::default()
    };

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
    server_app.insert_resource(TimeUpdateStrategy::ManualDuration(frame_duration));

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
    // TODO: maybe use ManualInstant for more granular control?
    client_app.insert_resource(TimeUpdateStrategy::ManualDuration(frame_duration));

    // Initialize Real time
    client_app
        .world
        .get_resource_mut::<Time<Real>>()
        .unwrap()
        .update_with_instant(Instant::now());
    server_app
        .world
        .get_resource_mut::<Time<Real>>()
        .unwrap()
        .update_with_instant(Instant::now());
    (client_app, server_app)
}
