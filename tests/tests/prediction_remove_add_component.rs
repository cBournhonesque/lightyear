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

use lightyear_shared::_reexport::*;
use lightyear_shared::prelude::client::*;
use lightyear_shared::prelude::*;
use lightyear_tests::protocol::{protocol, Channel2, Component1, MyInput, MyProtocol};
use lightyear_tests::stepper::{BevyStepper, Step};

fn increment_component(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Component1), With<Predicted>>,
) {
    for (entity, mut component) in query.iter_mut() {
        component.0 += 1.0;
        if component.0 == 5.0 {
            commands.entity(entity).remove::<Component1>();
        }
    }
}

fn setup() -> BevyStepper {
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
    let sync_config = SyncConfig::default().speedup_factor(1.0);
    let prediction_config = PredictionConfig::default().disable(false);
    let interpolation_delay = Duration::from_millis(100);
    let interpolation_config =
        InterpolationConfig::default().with_delay(InterpolationDelay::Delay(interpolation_delay));
    let mut stepper = BevyStepper::new(
        shared_config,
        sync_config,
        prediction_config,
        interpolation_config,
        link_conditioner,
        frame_duration,
    );
    stepper.client_mut().set_synced();
    stepper.client_app.add_systems(
        FixedUpdate,
        increment_component.in_set(FixedUpdateSet::Main),
    );
    stepper
}

// Test that if a component gets removed from the predicted entity erroneously
// We are still able to rollback properly (the rollback adds the component to the predicted entity)
#[test]
fn test_removed_predicted_component_rollback() -> anyhow::Result<()> {
    let mut stepper = setup();

    // Create a confirmed entity
    let confirmed = stepper
        .client_app
        .world
        .spawn((Component1(0.0), ShouldBePredicted))
        .id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let predicted = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .predicted
        .unwrap();

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
    let mut history = PredictionHistory::<Component1>::new();
    // this is added during the first rollback call after we create the history
    history
        .buffer
        .add_item(Tick(0), ComponentState::Updated(Component1(0.0)));
    history
        .buffer
        .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
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
        &Component1(1.0)
    );

    // advance five more frames, so that the component gets removed on predicted
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
    let mut history = PredictionHistory::<Component1>::new();
    for i in 0..5 {
        history
            .buffer
            .add_item(Tick(i), ComponentState::Updated(Component1(i as f32)));
    }
    history.buffer.add_item(Tick(5), ComponentState::Removed);
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history,
    );

    // create a rollback situation
    stepper.client_mut().set_synced();
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(3));
    stepper
        .client_app
        .world
        .get_mut::<Component1>(confirmed)
        .unwrap()
        .0 = 1.0;
    // update without incrementing time, because we want to force a rollback check
    stepper.client_app.update();

    // check that rollback happened
    // predicted got the component re-added
    stepper
        .client_app
        .world
        .get_mut::<Component1>(predicted)
        .unwrap()
        .0 = 4.0;
    // check that the history is how we expect after rollback
    let mut history = PredictionHistory::<Component1>::new();
    for i in 3..7 {
        history
            .buffer
            .add_item(Tick(i), ComponentState::Updated(Component1(i as f32 - 2.0)));
    }
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history
    );

    Ok(())
}

// Test that if a component gets added to the predicted entity erroneously but didn't exist on the confirmed entity)
// We are still able to rollback properly (the rollback removes the component from the predicted entity)
#[test]
fn test_added_predicted_component_rollback() -> anyhow::Result<()> {
    let mut stepper = setup();

    // Create a confirmed entity
    let confirmed = stepper.client_app.world.spawn(ShouldBePredicted).id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let predicted = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .predicted
        .unwrap();

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

    // add a new component to Predicted
    stepper
        .client_app
        .world
        .entity_mut(predicted)
        .insert(Component1(1.0));

    // create a rollback situation (confirmed doesn't have a component that predicted has)
    stepper.client_mut().set_synced();
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(1));
    // update without incrementing time, because we want to force a rollback check
    stepper.client_app.update();

    // check that rollback happened: the component got removed from predicted
    assert!(stepper
        .client_app
        .world
        .get::<Component1>(predicted)
        .is_none());

    // check that history contains the removal
    let mut history = PredictionHistory::<Component1>::new();
    history.buffer.add_item(Tick(1), ComponentState::Removed);
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history,
    );
    Ok(())
}

// Test that if a component gets removed from the confirmed entity
// We are still able to rollback properly (the rollback removes the component from the predicted entity)
#[test]
fn test_removed_confirmed_component_rollback() -> anyhow::Result<()> {
    let mut stepper = setup();

    // Create a confirmed entity
    let confirmed = stepper
        .client_app
        .world
        .spawn((Component1(0.0), ShouldBePredicted))
        .id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let predicted = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .predicted
        .unwrap();

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
    let mut history = PredictionHistory::<Component1>::new();
    history
        .buffer
        .add_item(Tick(0), ComponentState::Updated(Component1(0.0)));
    history
        .buffer
        .add_item(Tick(1), ComponentState::Updated(Component1(1.0)));
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history,
    );

    // create a rollback situation by removing the component on confirmed
    stepper.client_mut().set_synced();
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(1));
    stepper
        .client_app
        .world
        .entity_mut(confirmed)
        .remove::<Component1>();
    // update without incrementing time, because we want to force a rollback check
    // (need duration_since_latest_received_server_tick = 0)
    stepper.client_app.update();

    // check that rollback happened
    // predicted got the component removed
    assert!(stepper
        .client_app
        .world
        .get_mut::<Component1>(predicted)
        .is_none());

    // check that the history is how we expect after rollback
    let mut history = PredictionHistory::<Component1>::new();
    history.buffer.add_item(Tick(1), ComponentState::Removed);
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history
    );

    Ok(())
}

// Test that if a component gets added to the confirmed entity (but didn't exist on the predicted entity)
// We are still able to rollback properly (the rollback adds the component to the predicted entity)
#[test]
fn test_added_confirmed_component_rollback() -> anyhow::Result<()> {
    let mut stepper = setup();

    // Create a confirmed entity
    let confirmed = stepper.client_app.world.spawn(ShouldBePredicted).id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let predicted = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .predicted
        .unwrap();

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

    // check that the component history did not get created
    assert!(stepper
        .client_app
        .world
        .get::<PredictionHistory<Component1>>(predicted)
        .is_none());

    // advance five more frames, so that the component gets removed on predicted
    for i in 0..5 {
        stepper.frame_step();
    }
    assert_eq!(stepper.client().tick(), Tick(6));

    // create a rollback situation by adding the component on confirmed
    stepper.client_mut().set_synced();
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(3));
    stepper
        .client_app
        .world
        .entity_mut(confirmed)
        .insert(Component1(1.0));
    // update without incrementing time, because we want to force a rollback check
    stepper.client_app.update();

    // check that rollback happened
    // predicted got the component re-added
    stepper
        .client_app
        .world
        .get_mut::<Component1>(predicted)
        .unwrap()
        .0 = 4.0;
    // check that the history is how we expect after rollback
    let mut history = PredictionHistory::<Component1>::new();
    for i in 3..7 {
        history
            .buffer
            .add_item(Tick(i), ComponentState::Updated(Component1(i as f32 - 2.0)));
    }
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<PredictionHistory<Component1>>(predicted)
            .unwrap(),
        &history
    );

    Ok(())
}
