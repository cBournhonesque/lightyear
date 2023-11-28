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

use lightyear_shared::netcode::generate_key;
use lightyear_shared::prelude::client::*;
use lightyear_shared::prelude::*;
use lightyear_tests::protocol::{protocol, Channel2, Component1, Component2, MyInput, MyProtocol};
use lightyear_tests::stepper::{BevyStepper, Step};

fn setup() -> (BevyStepper, Entity, Entity, u16) {
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
    let prediction_config = PredictionConfig::default().disable(true);
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

    // Create a confirmed entity
    let confirmed = stepper
        .client_app
        .world
        .spawn((Component1(0.0), ShouldBeInterpolated))
        .id();

    // Tick once
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(1));
    let interpolated = stepper
        .client_app
        .world
        .get::<Confirmed>(confirmed)
        .unwrap()
        .interpolated
        .unwrap();

    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(confirmed)
            .unwrap(),
        &Component1(0.0)
    );

    // check that the interpolated entity got spawned
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Interpolated>(interpolated)
            .unwrap()
            .confirmed_entity,
        confirmed
    );

    // check that the component history got created and is empty
    let history = ConfirmedHistory::<Component1>::new();
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<ConfirmedHistory<Component1>>(interpolated)
            .unwrap(),
        &history,
    );
    // check that the confirmed component got replicated
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(interpolated)
            .unwrap(),
        &Component1(0.0)
    );
    // check that the interpolate status got updated
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<InterpolateStatus<Component1>>(interpolated)
            .unwrap(),
        &InterpolateStatus::<Component1> {
            start: None,
            end: (Tick(0), Component1(0.0)).into(),
            current: Tick(1) - interpolation_tick_delay,
        }
    );
    (stepper, confirmed, interpolated, interpolation_tick_delay)
}

// Test interpolation
#[test]
fn test_interpolation() -> anyhow::Result<()> {
    let (mut stepper, confirmed, interpolated, interpolation_tick_delay) = setup();
    // reach interpolation start tick
    stepper.frame_step();
    stepper.frame_step();
    // check that the interpolate status got updated (end becomes start)
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<InterpolateStatus<Component1>>(interpolated)
            .unwrap(),
        &InterpolateStatus::<Component1> {
            start: (Tick(0), Component1(0.0)).into(),
            end: None,
            current: Tick(3) - interpolation_tick_delay,
        }
    );

    // receive server update
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(2));
    stepper
        .client_app
        .world
        .get_entity_mut(confirmed)
        .unwrap()
        .get_mut::<Component1>()
        .unwrap()
        .0 = 2.0;

    stepper.frame_step();
    // check that interpolation is working correctly
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<InterpolateStatus<Component1>>(interpolated)
            .unwrap(),
        &InterpolateStatus::<Component1> {
            start: (Tick(0), Component1(0.0)).into(),
            end: (Tick(2), Component1(2.0)).into(),
            current: Tick(4) - interpolation_tick_delay,
        }
    );
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(interpolated)
            .unwrap(),
        &Component1(1.0)
    );
    stepper.frame_step();
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<InterpolateStatus<Component1>>(interpolated)
            .unwrap(),
        &InterpolateStatus::<Component1> {
            start: (Tick(2), Component1(2.0)).into(),
            end: None,
            current: Tick(5) - interpolation_tick_delay,
        }
    );
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(interpolated)
            .unwrap(),
        &Component1(2.0)
    );
    Ok(())
}

// We are in the situation: S1 < I
// where S1 is a confirmed ticks, and I is the interpolated tick
// and we receive S1 < S2 < I
// Then we should now start interpolating from S2
#[test]
fn test_received_more_recent_start() -> anyhow::Result<()> {
    let (mut stepper, confirmed, interpolated, interpolation_tick_delay) = setup();

    // reach interpolation start tick
    stepper.frame_step();
    stepper.frame_step();
    stepper.frame_step();
    stepper.frame_step();
    assert_eq!(stepper.client().tick(), Tick(5));

    // receive server update
    stepper
        .client_mut()
        .set_latest_received_server_tick(Tick(1));
    stepper
        .client_app
        .world
        .get_entity_mut(confirmed)
        .unwrap()
        .get_mut::<Component1>()
        .unwrap()
        .0 = 1.0;

    stepper.frame_step();
    // check the status uses the more recent server update
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<InterpolateStatus<Component1>>(interpolated)
            .unwrap(),
        &InterpolateStatus::<Component1> {
            start: (Tick(1), Component1(1.0)).into(),
            end: None,
            current: Tick(6) - interpolation_tick_delay,
        }
    );
    Ok(())
}
