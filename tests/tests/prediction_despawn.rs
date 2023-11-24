#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use bevy::log::LogPlugin;
use bevy::prelude::{
    App, Commands, Entity, EventReader, EventWriter, FixedUpdate, IntoSystemConfigs, PluginGroup,
    Query, Real, Res, ResMut, Startup, Time, With,
};
use bevy::time::TimeUpdateStrategy;
use bevy::winit::WinitPlugin;
use bevy::{DefaultPlugins, MinimalPlugins};
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_shared::client::components::Confirmed;
use lightyear_shared::client::interpolation::plugin::{InterpolationConfig, InterpolationDelay};
use lightyear_shared::client::prediction::plugin::PredictionConfig;
use lightyear_shared::client::prediction::{
    ComponentState, Predicted, PredictionCommandsExt, PredictionDespawnMarker, PredictionHistory,
    ShouldBePredicted,
};
use lightyear_shared::client::{Authentication, Client, ClientConfig, InputSystemSet, SyncConfig};
use lightyear_shared::netcode::generate_key;
use lightyear_shared::plugin::events::InputEvent;
use lightyear_shared::plugin::sets::FixedUpdateSet;
use lightyear_shared::replication::{NetworkTarget, Replicate};
use lightyear_shared::server::{NetcodeConfig, PingConfig, Server, ServerConfig};
use lightyear_shared::tick::Tick;
use lightyear_shared::{
    ChannelKind, ClientId, IoConfig, LinkConditionerConfig, MainSet, SharedConfig, TickConfig,
    TransportConfig,
};
use lightyear_tests::protocol::{protocol, Channel2, Component1, MyInput, MyProtocol};
use lightyear_tests::stepper::{BevyStepper, Step};

fn increment_component_and_despawn(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Component1), With<Predicted>>,
) {
    for (entity, mut component) in query.iter_mut() {
        component.0 += 1.0;
        if component.0 == 5.0 {
            commands.entity(entity).prediction_despawn::<MyProtocol>();
        }
    }
}

// Test that if a predicted entity gets despawned erroneously
// We are still able to rollback properly (the rollback re-adds the predicted entity, or prevents it from despawning)
#[test]
fn test_despawned_predicted_rollback() -> anyhow::Result<()> {
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
    let interpolation_tick_delay = 3;
    let interpolation_config = InterpolationConfig::default()
        .with_delay(InterpolationDelay::Ticks(interpolation_tick_delay));
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
        increment_component_and_despawn.in_set(FixedUpdateSet::Main),
    );

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
    // // check that predicted has the despawn marker
    // assert_eq!(
    //     stepper
    //         .client_app
    //         .world
    //         .get::<PredictionDespawnMarker>(predicted)
    //         .unwrap(),
    //     &PredictionDespawnMarker {
    //         death_tick: Tick(5)
    //     }
    // );
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
    // predicted exists, and got the component re-added
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

// Test that if another entity gets added during prediction,
// - either it should get despawned if there is a rollback that doesn't add it anymore
// - or we should just let it live? (imagine it's audio, etc.)

fn increment_component_and_despawn_both(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Component1)>,
) {
    for (entity, mut component) in query.iter_mut() {
        component.0 += 1.0;
        if component.0 == 5.0 {
            commands.entity(entity).prediction_despawn::<MyProtocol>();
        }
    }
}

// Test that if a confirmed entity gets despawned,
// the corresponding predicted entity gets despawned as well
// Test that if a predicted entity gets despawned erroneously
// We are still able to rollback properly (the rollback re-adds the predicted entity, or prevents it from despawning)
#[test]
fn test_despawned_confirmed_rollback() -> anyhow::Result<()> {
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
    let interpolation_tick_delay = 3;
    let interpolation_config = InterpolationConfig::default()
        .with_delay(InterpolationDelay::Ticks(interpolation_tick_delay));
    let mut stepper = BevyStepper::new(
        shared_config,
        sync_config,
        prediction_config,
        interpolation_config,
        link_conditioner,
        frame_duration,
    );
    stepper.client_app.add_systems(
        FixedUpdate,
        increment_component_and_despawn_both.in_set(FixedUpdateSet::Main),
    );

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

    // create a situation where the confirmed entity gets despawned during FixedUpdate::Main
    stepper.client_mut().set_synced();
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(0));
    // we set it to 5 so that it gets despawned during FixedUpdate::Main
    stepper
        .client_app
        .world
        .get_mut::<Component1>(confirmed)
        .unwrap()
        .0 = 4.0;
    // update without incrementing time, because we want to force a rollback check
    stepper.frame_step();

    // check that rollback happened
    // confirmed and predicted both got despawned
    assert!(stepper.client_app.world.get_entity(confirmed).is_none());
    assert!(stepper.client_app.world.get_entity(predicted).is_none());

    Ok(())
}
