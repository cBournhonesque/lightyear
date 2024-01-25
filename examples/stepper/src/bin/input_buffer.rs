#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use bevy::utils::{Duration, Instant};
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::log::LogPlugin;
use bevy::prelude::{
    App, Commands, EventReader, FixedUpdate, IntoSystemConfigs, PluginGroup, Real, Res, ResMut,
    Startup, Time,
};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear::prelude::client::{Authentication, ClientConfig, InputSystemSet, SyncConfig};
use lightyear::prelude::server::{NetcodeConfig, ServerConfig};
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

// System that runs every fixed timestep, and will add an input to the buffer
fn buffer_client_inputs(mut client: ResMut<Client>) {
    let tick = client.tick();
    client.add_input(MyInput(tick.0 as i16))
}

fn client_read_input(
    client: Res<Client>,
    mut input_reader: EventReader<client::InputEvent<MyInput>>,
) {
    for input in input_reader.read() {
        info!(
            "Client has input {:?} at tick {:?}",
            input.input(),
            client.tick()
        );
    }
}

fn server_read_input(
    // TODO: maybe put the tick in a separate resource? it lowers parallelism to have to fetch the entire server just to get the tick..
    server: Res<Server>,
    mut input_reader: EventReader<server::InputEvent<MyInput>>,
) {
    let tick = server.tick();
    for input in input_reader.read() {
        if input.input().is_some() {
            info!(
                "Server received input {:?} from client {:?} at tick {:?}",
                input.input(),
                input.context(),
                tick
            );
        }
    }
}

fn main() -> anyhow::Result<()> {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        enable_replication: false,
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(20),
        incoming_jitter: Duration::from_millis(0),
        incoming_loss: 0.0,
    };
    let mut stepper = BevyStepper::new(
        shared_config,
        SyncConfig::default(),
        client::PredictionConfig::default(),
        client::InterpolationConfig::default(),
        link_conditioner,
        frame_duration,
    );

    // add systems
    stepper.client_app.add_systems(Startup, client_init);
    stepper.server_app.add_systems(Startup, server_init);
    stepper.client_app.add_systems(
        FixedUpdate,
        buffer_client_inputs.in_set(InputSystemSet::BufferInputs),
    );
    stepper
        .client_app
        .add_systems(FixedUpdate, client_read_input.in_set(FixedUpdateSet::Main));
    stepper
        .server_app
        .add_systems(FixedUpdate, server_read_input.in_set(FixedUpdateSet::Main));

    // tick a bit, and check the input buffer received on server
    for i in 0..400 {
        stepper.frame_step();
    }

    // TODO: add asserts? at least we correctly receive inputs!

    // TODO:
    //  -Sometimes, the client's InputMessage has some absent inputs in the middle for some reason ??
    //     - not sure if it still happens
    //  -check on client behaves during rollback (need to use rollback tick)

    Ok(())
}
