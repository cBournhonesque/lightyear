#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::log::LogPlugin;
use bevy::prelude::{App, Commands, PluginGroup, Real, ResMut, Startup, Time};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_client::{Authentication, Client, ClientConfig};
use lightyear_server::{NetcodeConfig, PingConfig, ServerConfig};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::replication::Replicate;
use lightyear_shared::tick::Tick;
use lightyear_shared::{
    ChannelKind, IoConfig, LinkConditionerConfig, SharedConfig, TickConfig, TransportConfig,
};
use lightyear_tests::protocol::{protocol, Channel2, MyProtocol};
use lightyear_tests::utils::{client, server, tick};

fn client_init(mut client: ResMut<Client<MyProtocol>>) {
    info!("Connecting to server");
    client.connect();
}

fn server_init(mut commands: Commands) {
    info!("Spawning entity on server");
    commands.spawn(Replicate {
        channel: ChannelKind::of::<Channel2>(),
        ..Default::default()
    });
}

// fn server_init(world: &mut World) {
//     info!("Spawning entity on server");
//     std::thread::sleep(Duration::from_secs(1));
//     let replicate = Replicate::<Channel2>::default();
//     let entity = world.spawn(replicate.clone()).id();
//     let mut server = world.resource_mut::<Server<MyProtocol>>();
//     server.entity_spawn(entity, vec![], &replicate).unwrap();
// }

#[test]
fn test_bevy_step() -> anyhow::Result<()> {
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
    // TODO: link conditioner doesn't work with virtual time
    let conditioner = LinkConditionerConfig {
        incoming_latency: 45,
        incoming_jitter: 3,
        incoming_loss: 0.0,
    };

    // Setup server
    let mut server_app = App::new();
    server_app.add_plugins(MinimalPlugins.build());
    let netcode_config = NetcodeConfig::default()
        .with_protocol_id(protocol_id)
        .with_key(private_key);
    let config = ServerConfig {
        netcode: netcode_config,
        io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
            .with_conditioner(conditioner.clone()),
        tick: TickConfig::new(fixed_timestep),
        ping: PingConfig::default(),
    };
    let plugin_config = lightyear_server::PluginConfig::new(config, protocol());
    let plugin = lightyear_server::Plugin::new(plugin_config);
    server_app.add_plugins(plugin);
    server_app.add_systems(Startup, server_init);
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
        shared: SharedConfig::default(),
        netcode: Default::default(),
        io: IoConfig::from_transport(TransportConfig::UdpSocket(addr))
            .with_conditioner(conditioner.clone()),
        tick: TickConfig::new(fixed_timestep),
        ping: lightyear_client::PingConfig {
            sync_num_pings: 10,
            sync_ping_interval_ms: Duration::from_millis(30),
            ping_interval_ms: Default::default(),
            rtt_ms_initial_estimate: Default::default(),
            jitter_ms_initial_estimate: Default::default(),
            rtt_smoothing_factor: 0.0,
        },
    };
    let plugin_config = lightyear_client::PluginConfig::new(config, protocol(), auth);
    let plugin = lightyear_client::Plugin::new(plugin_config);
    client_app.add_plugins(plugin);
    client_app.add_systems(Startup, client_init);
    // TODO: maybe use ManualInstant for more granular control?
    client_app.insert_resource(TimeUpdateStrategy::ManualDuration(frame_duration));

    macro_rules! tick_once {
        () => {
            mock_instant::MockClock::advance(frame_duration);
            tick(&mut client_app);
            tick(&mut server_app);
        };
    }

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

    // app start: check that tick increment works
    tick_once!();
    assert_eq!(client(&mut client_app).tick(), Tick(1));
    tick_once!();
    assert_eq!(client(&mut client_app).tick(), Tick(3));

    // check that time sync works
    for i in 0..60 {
        tick_once!();
    }
    // check that connection is synced?
    // assert_eq!(client(&mut client_app).inc

    // TODO:
    // check that delta-tick works and that speedup/slowdown works
    // (for example set a delta-tick too low and the client packets arrive too late)

    Ok(())
}
