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

use lightyear_shared::client::prediction::Predicted;
use lightyear_shared::client::{Authentication, Client, ClientConfig, InputSystemSet};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::plugin::events::InputEvent;
use lightyear_shared::plugin::sets::FixedUpdateSet;
use lightyear_shared::replication::Replicate;
use lightyear_shared::server::{NetcodeConfig, PingConfig, Server, ServerConfig};
use lightyear_shared::tick::Tick;
use lightyear_shared::{
    ChannelKind, ClientId, IoConfig, LinkConditionerConfig, MainSet, SharedConfig, TickConfig,
    TransportConfig,
};
use lightyear_tests::protocol::{protocol, Channel2, Component1, MyInput, MyProtocol};
use lightyear_tests::stepper::{BevyStepper, Step};
use lightyear_tests::tick_once;
use lightyear_tests::utils::{init_bevy_step, tick};

fn client_init(mut client: ResMut<Client<MyProtocol>>) {
    info!("Connecting to server");
    client.connect();
}

fn server_init(mut commands: Commands) {
    info!("Spawning entity on server");
    commands.spawn((
        Replicate {
            channel: ChannelKind::of::<Channel2>(),
            should_do_prediction: true,
            ..Default::default()
        },
        Component1(0),
    ));
}

// System that runs every fixed timestep, and will add an input to the buffer
fn buffer_client_inputs(mut client: ResMut<Client<MyProtocol>>) {
    let tick = client.tick();
    let amplitude = 10 as i16;
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
    component1.0 += input.0;
}

// The client input only gets applied to predicted entities
fn client_read_input(
    client: Res<Client<MyProtocol>>,
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
    server: Res<Server<MyProtocol>>,
    mut component1_query: Query<&mut Component1>,
    mut input_reader: EventReader<InputEvent<MyInput, ClientId>>,
) {
    let tick = server.tick();
    for input in input_reader.read() {
        if input.input().is_some() {
            let input = input.input().as_ref().unwrap();
            // apply physics
            for mut component1 in component1_query.iter_mut() {
                info!(
                    "Server updated component1 {:?} with input {:?}, at tick {:?}",
                    &component1, input, tick
                );
                shared_behaviour(&mut component1, input);
            }
        }
    }
}

#[test]
fn test_bevy_step_prediction() -> anyhow::Result<()> {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        enable_replication: false,
        tick: TickConfig::new(tick_duration),
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: 0,
        incoming_jitter: 0,
        incoming_loss: 0.0,
    };
    let mut stepper = BevyStepper::new(shared_config, link_conditioner, frame_duration);

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

    // TODO: add asserts? at least we correctly receive inputs!

    // TODO:
    //  - at 0 latency, the server/client don't match but we don't do rollbacks. Why?
    //  - the confirmed Component1 state when we initially check rollback is 0 for some reason, is it because we didn't receive the update?
    //    we receive Component1 = 0 when server-tick = 15, doesn't seem to make sense?
    //    oh it's because the client sends inputs only after time-synced, so the server only starts using inputs after time sync
    //    => MAYBE ON CLIENT WE SHOULDN'T RECEIVE REPLICATION EVENTS UNTIL TIME SYNCED?
    //  - it looks like we keep doing a lot of small rollbacks when the ticks become slightly out of sync by 1 (because of tick boundaries)?
    //      its probably because of we update the latest_received_server_tick from another packet (like ping), but we didn't receive the latest
    //      update for the component yet. So the latest_received_server_tick and confirmed_component_value are no in sync.
    //      this is the crucial potential bug that we outlined. Should we take the latest tick of each latest component update?
    //      or just the latest server-received tick, across everything?
    //      Confirm that this is the cause, and think about the solution.
    //      Use tick-buffered for EntityUpdates?
    //      Or maybe that's intended? that's why we have rollback? but then that's a lot of rollback

    Ok(())
}
