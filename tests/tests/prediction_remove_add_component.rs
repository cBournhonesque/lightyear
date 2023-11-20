#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::log::LogPlugin;
use bevy::prelude::{
    App, Commands, Entity, EventReader, FixedUpdate, IntoSystemConfigs, PluginGroup, Query, Real,
    Res, ResMut, Startup, Time, With,
};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_shared::client::prediction::{
    ComponentHistory, ComponentState, Confirmed, Predicted, ShouldBePredicted,
};
use lightyear_shared::client::{Authentication, Client, ClientConfig, InputSystemSet};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::plugin::events::InputEvent;
use lightyear_shared::plugin::sets::FixedUpdateSet;
use lightyear_shared::replication::{PredictionTarget, Replicate};
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

fn increment_component(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Component1), With<Predicted>>,
) {
    for (entity, mut component) in query.iter_mut() {
        component.0 += 1;
        if component.0 == 5 {
            commands.entity(entity).remove::<Component1>();
        }
    }
}

#[test]
fn test_prediction_remove_add_component() -> anyhow::Result<()> {
    let frame_duration = Duration::from_millis(10);
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
    let mut stepper = BevyStepper::new(shared_config, link_conditioner, frame_duration);
    stepper.client_app.add_systems(
        FixedUpdate,
        increment_component.in_set(FixedUpdateSet::Main),
    );

    // Create a confirmed entity
    let confirmed = stepper
        .client_app
        .world
        .spawn((Component1(0), ShouldBePredicted))
        .id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let predicted = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .predicted;

    // check that the predicted entity got spawned
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Predicted>(predicted)
            .unwrap()
            .confirmed_entity,
        confirmed
    );

    // check that the component history got created
    let mut history = ComponentHistory::<Component1>::new();
    history
        .buffer
        .add_item(Tick(1), ComponentState::Added(Component1(1)));
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<ComponentHistory<Component1>>(predicted)
            .unwrap(),
        &history,
    );
    // check that the confirmed component got replicated
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(predicted)
            .unwrap(),
        &Component1(1)
    );

    // advance five more frames
    for i in 0..5 {
        stepper.frame_step();
    }
    assert_eq!(stepper.client().tick(), Tick(6));

    // check that the component got removed on predicted
    assert!(stepper
        .client_app
        .world
        .get::<Component1>(predicted)
        .is_none());
    // check that the component history is still there and that the value of the component history is correct
    let mut history = ComponentHistory::<Component1>::new();
    history
        .buffer
        .add_item(Tick(1), ComponentState::Added(Component1(1 as i16)));
    for i in 2..5 {
        history
            .buffer
            .add_item(Tick(i), ComponentState::Updated(Component1(i as i16)));
    }
    history.buffer.add_item(Tick(5), ComponentState::Removed);
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<ComponentHistory<Component1>>(predicted)
            .unwrap(),
        &history,
    );

    // create a rollback situation
    // TODO: we are not setting duration_since_latest_received_server_tick to 0 during the schedule, so nofallback
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(3));
    stepper
        .client_app
        .world
        .get_mut::<Component1>(confirmed)
        .unwrap()
        .0 = 1;
    stepper.frame_step();

    // check that we rolled-back even though the component was removed on predicted

    Ok(())
}
