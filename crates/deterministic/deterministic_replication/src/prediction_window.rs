use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_core::timeline::LocalTimeline;
use lightyear_inputs::client::InputSystems;
use lightyear_prediction::prelude::{LastConfirmedInput, PredictionManager};
use lightyear_sync::plugin::SyncSystems;
use lightyear_sync::prelude::{InputTimelineConfig, PredictionWindowWait, SyncedLocalTimeline};
use tracing::{info, trace};

/// Installs the application-global deterministic prediction-window controller.
pub(crate) struct PredictionWindowWaitPlugin;

impl Plugin for PredictionWindowWaitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PredictionWindowWait>();
        app.init_resource::<LastConfirmedInput>();
        app.add_systems(
            PostUpdate,
            update_prediction_window_wait
                .after(InputSystems::UpdateRemoteInputTicks)
                .after(SyncSystems::Sync),
        );
    }
}

/// Stop deterministic fixed simulation before its next tick would exceed the rollback-safe input
/// window.
///
/// [`LastConfirmedInput`] is the minimum across every remote input buffer and every registered
/// input type. A missing stream anchors the controller to the session's first observed tick until
/// input arrives. An application with no remote input streams does not wait.
fn update_prediction_window_wait(
    timeline: SyncedLocalTimeline,
    metadata: Res<NetworkingMetadata>,
    input_config: Res<InputTimelineConfig>,
    prediction_manager: Option<Res<PredictionManager>>,
    last_confirmed_input: Res<LastConfirmedInput>,
    mut wait: ResMut<PredictionWindowWait>,
) {
    if metadata.is_changed() {
        wait.reset();
    }

    let active_topology = match &metadata.mode {
        NetworkTopology::Client(_) => true,
        NetworkTopology::P2P { connected, .. } => !connected.is_empty(),
        NetworkTopology::Undefined
        | NetworkTopology::Server(_)
        | NetworkTopology::HostClient { .. }
        | NetworkTopology::Invalid(_) => false,
    };
    let Some(prediction_manager) = prediction_manager.filter(|_| active_topology) else {
        wait.reset();
        return;
    };

    let current_frontier = last_confirmed_input.get();
    if current_frontier.is_none() && last_confirmed_input.received_for_all_clients {
        // An empty aggregate means this deterministic simulation has no remote input streams. A
        // stream that exists but has not received its first sample sets this flag to false.
        wait.reset();
        return;
    }

    let configured_maximum = input_config.maximum_predicted_ticks();
    let effective_maximum = if configured_maximum == 0 {
        // `effective_max_rollback_ticks` intentionally leaves forced state rollback enabled in
        // lockstep mode. The input prediction window itself is still zero.
        0
    } else {
        prediction_manager
            .rollback_policy
            .effective_max_rollback_ticks(&input_config)
    };
    let confirmed_tick = last_confirmed_input
        .received_for_all_clients
        .then_some(current_frontier)
        .flatten();
    let changed = wait.update(timeline.tick(), confirmed_tick, effective_maximum);

    if changed {
        info!(
            waiting = wait.is_waiting(),
            current_tick = timeline.tick().0,
            confirmed_tick = ?wait.confirmed_tick(),
            prediction_depth = wait.prediction_depth(),
            maximum_predicted_ticks = wait.maximum_predicted_ticks(),
            "deterministic prediction-window wait state changed"
        );
    }
    trace!(
        target: "lightyear_debug::input",
        kind = "prediction_window",
        schedule = "PostUpdate",
        sample_point = "PostUpdate",
        waiting = wait.is_waiting(),
        current_tick = timeline.tick().0,
        confirmed_tick = ?wait.confirmed_tick(),
        prediction_depth = wait.prediction_depth(),
        maximum_predicted_ticks = wait.maximum_predicted_ticks(),
        "updated deterministic prediction-window wait signal"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_core::prelude::Tick;
    use lightyear_sync::prelude::LocalTimelineSync;

    fn wait_app(topology: NetworkTopology) -> App {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<LocalTimelineSync>();
        app.init_resource::<InputTimelineConfig>();
        app.init_resource::<NetworkingMetadata>();
        app.init_resource::<LastConfirmedInput>();
        app.init_resource::<PredictionWindowWait>();
        app.insert_resource(PredictionManager {
            rollback_policy: lightyear_prediction::prelude::RollbackPolicy {
                max_rollback_ticks: 4,
                ..Default::default()
            },
            ..Default::default()
        });
        app.add_systems(PostUpdate, update_prediction_window_wait);

        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = topology;
        app
    }

    #[test]
    fn wait_uses_effective_rollback_depth_for_client_and_p2p() {
        for p2p in [false, true] {
            let mut topology_world = World::new();
            let link = topology_world.spawn_empty().id();
            let topology = if p2p {
                NetworkTopology::P2P {
                    connected: [link].into_iter().collect(),
                    declared_links: 1,
                }
            } else {
                NetworkTopology::Client(link)
            };
            let mut app = wait_app(topology);
            app.world_mut()
                .resource_mut::<LocalTimeline>()
                .apply_delta(104);
            {
                let mut confirmed = app.world_mut().resource_mut::<LastConfirmedInput>();
                confirmed.tick.set_if_lower(Tick(100));
                confirmed.received_for_all_clients = true;
            }

            app.world_mut().run_schedule(PostUpdate);
            let wait = app.world().resource::<PredictionWindowWait>();
            assert!(wait.is_waiting());
            assert_eq!(wait.maximum_predicted_ticks(), 4);
        }
    }

    #[test]
    fn deterministic_session_without_remote_input_streams_does_not_wait() {
        let mut topology_world = World::new();
        let link = topology_world.spawn_empty().id();
        let mut app = wait_app(NetworkTopology::Client(link));
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(100);
        app.world_mut()
            .resource_mut::<LastConfirmedInput>()
            .received_for_all_clients = true;

        app.world_mut().run_schedule(PostUpdate);
        assert!(!app.world().resource::<PredictionWindowWait>().is_waiting());
    }
}
