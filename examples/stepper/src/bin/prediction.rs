#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::log::LogPlugin;
use bevy::prelude::{
    App, Commands, EventReader, FixedUpdate, IntoSystemConfigs, PluginGroup, Query, Real, Res,
    ResMut, Startup, Time, With,
};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear::_reexport::*;
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
    commands.spawn((
        Replicate {
            prediction_target: NetworkTarget::All,
            ..Default::default()
        },
        Component1(0.0),
    ));
}

// System that runs every fixed timestep, and will add an input to the buffer
fn buffer_client_inputs(mut client: ResMut<Client>) {
    let tick = client.tick();
    let amplitude = 10i16;
    // oscillating value between 0 and 10
    let value =
        |tick: Tick| amplitude - (amplitude - (tick.0 % (2 * amplitude) as u16) as i16).abs();
    let prev_value = value(tick);
    let current_value = value(tick - 1);
    let delta = current_value - prev_value;
    // TODO: i cannot snap the value to check rollback, i must do a delta
    client.add_input(MyInput(delta))
}

// Shared behaviour: make the component value equal to the input value
// - Server: used to update components (which will be replicated)
// - Client: used for client-prediction/rollback
fn shared_behaviour(component1: &mut Component1, input: &MyInput) {
    component1.0 += input.0 as f32;
}

// The client input only gets applied to predicted entities
fn client_read_input(
    client: Res<Client>,
    mut component1_query: Query<&mut Component1, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<MyInput>>,
) {
    for input in input_reader.read() {
        if input.input().is_some() {
            let input = input.input().as_ref().unwrap();
            for mut component1 in component1_query.iter_mut() {
                shared_behaviour(&mut component1, input);
                info!(
                    "Client updated component1 {:?} with input {:?}, at tick {:?}",
                    &component1,
                    input,
                    client.tick()
                );
            }
        }
    }
}

fn server_read_input(
    server: Res<Server>,
    mut component1_query: Query<&mut Component1>,
    mut input_reader: EventReader<server::InputEvent<MyInput>>,
) {
    let tick = server.tick();
    for input in input_reader.read() {
        if input.input().is_some() {
            let input = input.input().as_ref().unwrap();
            // apply physics
            for mut component1 in component1_query.iter_mut() {
                shared_behaviour(&mut component1, input);
                info!(
                    "Server updated component1 {:?} with input {:?}, at tick {:?}",
                    &component1, input, tick
                );
            }
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
        incoming_latency: Duration::from_millis(40),
        incoming_jitter: Duration::from_millis(5),
        incoming_loss: 0.05,
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
    for i in 0..200 {
        stepper.frame_step();
    }

    // TODO: add asserts

    // NOTE: situation
    // - rollback seems to work fairly well with only 1 player (only rollback and then we're in sync)
    //   - this is expected since inconsistencies happen mostly when we didn't predict other players' inputs

    // TODO: Sometimes there is a desync and we have to do another rollback, I don't really understand why

    // TODO: its possible that we update the latest_received_server_tick from another packet (like ping), but we didn't receive the latest
    //      update for the component yet. So the latest_received_server_tick and confirmed_component_value are no in sync.
    //      this is the crucial potential bug that we outlined. Should we take the latest tick of each latest component update?
    //      IS THIS REALLY THE CASE?
    //      we could keep track of latest-server-update-tick, i.e. the latest tick for the entire game state!

    Ok(())
}
