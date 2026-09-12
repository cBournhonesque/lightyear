//! A session that starts with a single local player.
//!
//! Solo play is the smallest dynamic membership case: the session must run, and stay usable, with
//! an empty remote roster, because that is also the state a host is in immediately before it
//! admits its first joiner.

use bevy::MinimalPlugins;
use bevy::prelude::{App, On, ResMut, Resource, Time, TransformPlugin, Virtual};
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use core::time::Duration;
use lightyear::p2p::{P2PSession, P2PSessionState, P2PStart, P2PStarted};
use lightyear::prediction::manager::LastConfirmedInput;
use lightyear::prelude::client::ClientPlugins;
use lightyear::prelude::*;
use test_log::test;

const TICK_DURATION: Duration = Duration::from_millis(10);

#[derive(Resource, Default)]
struct StartedTicks(Vec<Tick>);

fn record_started(trigger: On<P2PStarted>, mut started: ResMut<StartedTicks>) {
    started.0.push(trigger.start_tick);
}

/// A session with no remote Links starts, simulates, and reports a ready topology.
///
/// The topology assertion is the important one: reporting `Undefined` for an empty roster would
/// leave the solo peer without an input route, so it could neither capture nor apply its own input
/// and the start barrier could never complete.
#[test]
fn a_solo_session_starts_and_keeps_running() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, StatesPlugin));
    app.add_plugins(ClientPlugins {
        tick_duration: TICK_DURATION,
    });
    app.init_resource::<LastConfirmedInput>();
    app.insert_resource(P2PSession::default().with_min_players(1));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK_DURATION));
    app.init_resource::<StartedTicks>();
    app.add_observer(record_started);
    app.finish();
    app.cleanup();

    // A single peer declares no P2P Link at all.
    app.world_mut().trigger(P2PStart::default());

    // The barrier needs a synchronized input timeline and enough ticks to reach its agreed start
    // tick (the configured lead), so drive it well past both.
    for _ in 0..400 {
        app.update();
        if app.world().resource::<P2PSession>().is_started() {
            break;
        }
    }

    assert!(
        app.world().resource::<P2PSession>().is_started(),
        "a solo peer must be able to start a session on its own"
    );

    let started = app.world().resource::<StartedTicks>().0.clone();
    assert_eq!(started.len(), 1, "P2PStarted must fire exactly once");
    let start_tick = started[0];

    // The application becomes a started peer the moment it crosses the barrier, and a started
    // peer with no remote Link still has a ready P2P topology.
    app.update();
    assert_eq!(
        app.world().resource::<NetworkingMetadata>().mode,
        NetworkTopology::P2P(P2PRoster::from_started_links([])),
        "a solo session is a ready P2P topology with an empty roster"
    );
    assert!(
        app.world().resource::<LocalTimelineSync>().is_synced(),
        "a peer with no remote clock must still expose a synchronized timeline"
    );

    // The simulation must keep advancing: a solo peer has no remote input to wait for.
    for _ in 0..20 {
        app.update();
    }
    assert!(
        app.world().resource::<LocalTimeline>().tick() > start_tick,
        "a solo session must keep advancing past its start tick"
    );
    assert!(
        app.world().resource::<Time<Virtual>>().relative_speed() > 0.0,
        "a solo session must not be paused by the prediction-window wait"
    );
}

/// The default policy requires an opponent, so an empty cohort is refused.
#[test]
fn a_solo_start_is_refused_by_default() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, StatesPlugin));
    app.add_plugins(ClientPlugins {
        tick_duration: TICK_DURATION,
    });
    app.init_resource::<LastConfirmedInput>();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK_DURATION));
    app.finish();
    app.cleanup();

    app.world_mut().trigger(P2PStart::default());
    for _ in 0..20 {
        app.update();
    }

    assert_eq!(
        app.world().resource::<P2PSession>().state(),
        P2PSessionState::Stopped
    );
}
