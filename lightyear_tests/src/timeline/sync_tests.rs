use crate::stepper::{ClientServerStepper, StepperConfig};
use core::time::Duration;
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::InterpolationTimeline;
use lightyear_core::tick::Tick;
use lightyear_core::time::TickInstant;
use lightyear_core::timeline::NetworkTimeline;
use lightyear_sync::prelude::client::{InputDelayConfig, RemoteTimeline};
use lightyear_sync::prelude::{InputTimeline, InputTimelineConfig, PingManager, SyncConfig};
use lightyear_sync::timeline::sync::{SyncTargetTimeline, SyncedTimeline};

const TICK_DURATION: Duration = Duration::from_millis(10);

fn remote_timeline_at(tick: u32) -> RemoteTimeline {
    let mut remote = RemoteTimeline::default();
    remote.set_now(TickInstant::from(Tick(tick)));
    remote
}

fn ping_manager(rtt: Duration, jitter: Duration) -> PingManager {
    let mut ping_manager = PingManager::default();
    ping_manager.pongs_recv = 1;
    ping_manager.rtt_estimator_ewma.final_stats.rtt = rtt;
    ping_manager.rtt_estimator_ewma.final_stats.jitter = jitter;
    ping_manager
}

fn assert_tick_instant_close(actual: TickInstant, expected: TickInstant) {
    let error = (actual - expected).to_f32().abs();
    assert!(
        error < 0.001,
        "expected {expected:?}, got {actual:?}, error {error}"
    );
}

#[test_log::test]
fn input_timeline_objective_tracks_remote_latency_margin() {
    let remote = remote_timeline_at(100);
    let ping_manager = ping_manager(Duration::from_millis(40), Duration::from_millis(5));
    let config = InputTimelineConfig::default().with_sync_config(SyncConfig {
        handshake_pings: 0,
        jitter_multiple: 2,
        jitter_margin: 1.0,
        error_margin: 0.75,
        ..Default::default()
    });

    let objective =
        InputTimeline::default().sync_objective(&remote, &config, &ping_manager, TICK_DURATION);

    // remote 100 + RTT/2 2 ticks + jitter margin 2 ticks + controller deadband 0.75 ticks.
    assert_tick_instant_close(objective, TickInstant::lit("104.75"));
    assert!(objective > remote.current_estimate());
}

#[test_log::test]
fn input_timeline_initial_sync_resyncs_to_objective() {
    let remote = remote_timeline_at(100);
    let ping_manager = ping_manager(Duration::ZERO, Duration::ZERO);
    let config = InputTimelineConfig::default().with_sync_config(SyncConfig {
        handshake_pings: 0,
        ..Default::default()
    });
    let mut timeline = InputTimeline::default();

    let tick_delta = timeline.sync(&remote, &config, &ping_manager, TICK_DURATION);

    assert_eq!(tick_delta, Some(102));
    assert!(timeline.is_synced());
    assert_tick_instant_close(timeline.now(), TickInstant::from(Tick(102)));
    assert_eq!(timeline.relative_speed(), 1.0);
}

#[test_log::test]
fn input_timeline_adjusts_speed_after_repeated_error() {
    let remote = remote_timeline_at(100);
    let ping_manager = ping_manager(Duration::ZERO, Duration::ZERO);
    let config = InputTimelineConfig::default().with_sync_config(SyncConfig {
        handshake_pings: 0,
        error_margin: 0.5,
        max_error_margin: 10.0,
        consecutive_errors_threshold: 2,
        speedup_factor: 1.1,
        ..Default::default()
    });
    let mut timeline = InputTimeline::default();

    timeline.sync(&remote, &config, &ping_manager, TICK_DURATION);
    timeline.set_now(TickInstant::from(Tick(104)));

    assert_eq!(
        timeline.sync(&remote, &config, &ping_manager, TICK_DURATION),
        None
    );
    assert_eq!(
        timeline.sync(&remote, &config, &ping_manager, TICK_DURATION),
        None
    );

    assert!(
        timeline.relative_speed() < 1.0,
        "timeline should slow down when it is ahead of the objective; speed={}",
        timeline.relative_speed()
    );
}

#[test_log::test]
fn input_delay_config_update_recomputes_public_delay() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.client_mut(0).insert(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(3)),
    );

    stepper.frame_step(1);

    let input_timeline = stepper.client(0).get::<InputTimeline>().unwrap();
    assert_eq!(input_timeline.context.input_delay(), 3);
}

#[test_log::test]
fn interpolation_timeline_objective_lags_remote_by_delay_margin() {
    let remote = remote_timeline_at(100);
    let ping_manager = ping_manager(Duration::ZERO, Duration::ZERO);
    let config = InterpolationConfig::default()
        .with_min_delay(Duration::from_millis(20))
        .with_send_interval_ratio(0.0);

    let objective = InterpolationTimeline::default().sync_objective(
        &remote,
        &config,
        &ping_manager,
        TICK_DURATION,
    );

    assert_tick_instant_close(objective, TickInstant::from(Tick(97)));
    assert!(objective < remote.current_estimate());
}
