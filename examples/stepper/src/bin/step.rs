#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::log::LogPlugin;
use bevy::prelude::default;
use bevy::prelude::{App, Commands, PluginGroup, Real, ResMut, Startup, Time};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear::prelude::client::*;
use lightyear::prelude::*;
use stepper::protocol::*;
use stepper::stepper::{BevyStepper, Step};

fn client_init(mut client: ResMut<Client>) {
    info!("Connecting to server");
    client.connect();
}

fn server_init(mut commands: Commands) {
    info!("Spawning entity on server");
    commands.spawn(Replicate {
        ..Default::default()
    });
}

fn main() -> anyhow::Result<()> {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        enable_replication: false,
        tick: TickConfig::new(tick_duration),
        ..default()
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(40),
        incoming_jitter: Duration::from_millis(20),
        incoming_loss: 0.0,
    };
    let mut stepper = BevyStepper::new(
        shared_config,
        SyncConfig::default(),
        PredictionConfig::default(),
        InterpolationConfig::default(),
        link_conditioner,
        frame_duration,
    );

    // add systems
    stepper.client_app.add_systems(Startup, client_init);
    stepper.server_app.add_systems(Startup, server_init);

    // app start: check that tick increment works
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(3));

    // check that time sync works
    for i in 0..500 {
        stepper.frame_step();
    }

    // check that connection is synced?
    // assert_eq!(client(&mut client_app).inc

    // TODO:
    // check that delta-tick works and that speedup/slowdown works
    // (for example set a delta-tick too low and the client packets arrive too late)

    Ok(())
}
