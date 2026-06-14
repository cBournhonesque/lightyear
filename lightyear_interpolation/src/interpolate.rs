use crate::registry::InterpolationRegistry;
use crate::timeline::InterpolationTimeline;
use bevy_ecs::component::Mutable;
use bevy_ecs::prelude::Has;
use bevy_ecs::prelude::*;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_utils::prelude::DebugName;
use lightyear_core::prelude::{ConfirmedHistory, ConfirmedState, Interpolated, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::diff_history::ConfirmedHistoryPatchReceiver;
use lightyear_sync::prelude::client::IsSynced;
#[allow(unused_imports)]
use tracing::{info, trace};

/// Compute the interpolation fraction
pub fn interpolation_fraction(start: Tick, end: Tick, current: Tick, overstep: f32) -> f32 {
    ((current - start) as f32 + overstep) / (end - start) as f32
}

/// Maintain the confirmed-history anchors used for interpolation.
///
/// The goal is to keep the freshest bracketing pair while updates are flowing,
/// then advance unchanged values using empty mutate ticks when explicit component updates stop.
pub(crate) fn update_confirmed_history<C: Component + Clone>(
    // TODO: handle multiple interpolation timelines
    // TODO: exclude host-server
    interpolation_registry: Res<InterpolationRegistry>,
    interpolation: Single<&InterpolationTimeline>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut query: Query<(Entity, &mut ConfirmedHistory<C>, Has<C>), With<Interpolated>>,
    mut commands: Commands,
) {
    let timeline = interpolation.into_inner();
    let server_complete_tick = checkpoints.last_confirmed_tick();
    let current_interpolate_tick = timeline.now().tick();
    for (entity, mut history, present) in query.iter_mut() {
        // Replicon's marker fns already ran before this system. If this component received an
        // explicit update or removal at the completed server tick T, `write_history` /
        // `remove_history` already recorded that exact tick and `push_unchanged(T)` returns None.
        //
        // Therefore, when the newest confirmed state is still an Updated value older than T,
        // mutate-message completeness tells us no update/removal for this component occurred
        // through T, so we can carry the newest value forward as unchanged.
        if let Some(server_complete_tick) = server_complete_tick
            && let Some(previous_newest_tick) = history.push_unchanged(server_complete_tick)
        {
            trace!(
                target: "lightyear_debug::interpolation",
                kind = "confirmed_history_unchanged_advance",
                schedule = "Update",
                sample_point = "Update",
                entity = ?entity,
                component = ?DebugName::type_name::<C>(),
                previous_newest_tick = previous_newest_tick.0,
                server_complete_tick = server_complete_tick.0,
                history_len = history.len(),
                "advanced unchanged interpolation history"
            );
        }

        // Smart drain: only pop when there are 3+ keyframes and the second-oldest
        // has already been passed. This keeps a [behind, newest] pair alive during
        // short loss gaps instead of collapsing immediately to a single keyframe.
        while history.len() >= 3
            && history
                .get_nth_tick(1)
                .is_some_and(|tick| tick <= current_interpolate_tick)
        {
            history.pop_present();
        }

        // Apply the history state for the current interpolation time to the live component set:
        // insert once the add/update tick becomes visible, remove once a removal tick is reached,
        // and otherwise leave the current component value alone.
        match interpolation_registry.sample(
            &history,
            current_interpolate_tick,
            timeline.overstep().to_f32(),
        ) {
            None | Some(ConfirmedState::Removed) if present => {
                commands.entity(entity).try_remove::<C>();
            }
            Some(ConfirmedState::Confirmed(value)) if !present => {
                commands.entity(entity).try_insert(value);
            }
            _ => {}
        }
    }
}

pub(crate) fn update_confirmed_history_diff<C>(
    interpolation_registry: Res<InterpolationRegistry>,
    interpolation: Single<&InterpolationTimeline>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut query: Query<
        (
            Entity,
            &mut ConfirmedHistory<C>,
            &mut ConfirmedHistoryPatchReceiver<C>,
            Has<C>,
        ),
        With<Interpolated>,
    >,
    mut commands: Commands,
) where
    C: Component + Clone + RepliconDiffable,
{
    let timeline = interpolation.into_inner();
    let server_complete_tick = checkpoints.last_confirmed_tick();
    let current_interpolate_tick = timeline.now().tick();
    for (entity, mut history, mut patch_receiver, present) in query.iter_mut() {
        if let Some(server_complete_tick) = server_complete_tick
            && !patch_receiver.has_pending_patch_at_tick(server_complete_tick)
            && let Some(previous_newest_tick) = history.push_unchanged(server_complete_tick)
        {
            trace!(
                target: "lightyear_debug::interpolation",
                kind = "confirmed_history_unchanged_advance",
                schedule = "Update",
                sample_point = "Update",
                entity = ?entity,
                component = ?DebugName::type_name::<C>(),
                previous_newest_tick = previous_newest_tick.0,
                server_complete_tick = server_complete_tick.0,
                history_len = history.len(),
                "advanced unchanged diff interpolation history"
            );
        }

        if !patch_receiver.has_pending_patches() {
            while history.len() >= 3
                && history
                    .get_nth_tick(1)
                    .is_some_and(|tick| tick <= current_interpolate_tick)
            {
                history.pop_present();
            }

            if let Some(server_complete_tick) = server_complete_tick {
                patch_receiver.clear_before_tick(server_complete_tick, &history);
            }
        }

        match interpolation_registry.sample(
            &history,
            current_interpolate_tick,
            timeline.overstep().to_f32(),
        ) {
            None | Some(ConfirmedState::Removed) if present => {
                commands.entity(entity).try_remove::<C>();
            }
            Some(ConfirmedState::Confirmed(value)) if !present => {
                commands.entity(entity).try_insert(value);
            }
            _ => {}
        }
    }
}

/// Apply interpolation for the component
pub(crate) fn interpolate<C: Component<Mutability = Mutable> + Clone>(
    interpolation_registry: Res<InterpolationRegistry>,
    timeline: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut query: Query<(Entity, &mut C, &ConfirmedHistory<C>), With<Interpolated>>,
    mut commands: Commands,
) {
    let interpolation_tick = timeline.tick();
    let interpolation_overstep = timeline.overstep().to_f32();
    for (entity, mut component, history) in query.iter_mut() {
        match interpolation_registry.sample(history, interpolation_tick, interpolation_overstep) {
            Some(ConfirmedState::Confirmed(interpolated)) => {
                trace!(
                    target: "lightyear_debug::interpolation",
                    kind = "interpolation_apply",
                    schedule = "Update",
                    sample_point = "Update",
                    component = ?DebugName::type_name::<C>(),
                    interpolation_tick = interpolation_tick.0,
                    interpolation_overstep,
                    history_len = history.len(),
                    "applied interpolation"
                );
                *component = interpolated;
            }
            Some(ConfirmedState::Removed) => {
                commands.entity(entity).try_remove::<C>();
            }
            None => {}
        }
    }
}

// #[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InterpolationRegistry;
    use alloc::vec;
    use bevy_app::{App, Update};
    use bevy_ecs::component::Component;
    use bevy_replicon::prelude::{Diffable as RepliconDiffable, RepliconTick};
    use lightyear_core::time::TickInstant;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::diff_history::ConfirmedHistoryPatchReceiver;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp(f32);

    impl RepliconDiffable for TestComp {
        type Patch = f32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn setup_app(current_tick: Tick, send_interval_ms: u64) -> App {
        let mut app = App::new();
        app.world_mut()
            .insert_resource(ReplicationCheckpointMap::default());
        app.world_mut()
            .insert_resource(InterpolationRegistry::default());

        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(current_tick));
        timeline.remote_send_interval = core::time::Duration::from_millis(send_interval_ms);
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));
        app
    }

    fn confirm_server_tick(app: &mut App, replicon_tick: u32, server_tick: Tick) {
        let replicon_tick = RepliconTick::new(replicon_tick);
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(replicon_tick, server_tick);
        checkpoints.record_last_confirmed_tick(replicon_tick);
    }

    fn set_interpolation_tick(app: &mut App, tick: Tick) {
        let mut timelines = app.world_mut().query::<&mut InterpolationTimeline>();
        let mut timeline = timelines.single_mut(app.world_mut()).unwrap();
        timeline.set_now(TickInstant::from(tick));
    }

    fn insert_confirmed_history(
        app: &mut App,
        entity: Entity,
        history: ConfirmedHistory<TestComp>,
    ) {
        app.world_mut()
            .entity_mut(entity)
            .insert((Interpolated, history));
    }

    #[test]
    fn update_confirmed_history_advances_to_latest_empty_mutate_tick_when_idle() {
        let mut app = setup_app(Tick(30), 40);
        app.add_systems(
            Update,
            (
                update_confirmed_history::<TestComp>,
                interpolate::<TestComp>,
            )
                .chain(),
        );
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);
        confirm_server_tick(&mut app, 1, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(component, &TestComp(10.0));
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_records_repeated_empty_mutate_ticks() {
        let mut app = setup_app(Tick(25), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);
        confirm_server_tick(&mut app, 1, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        confirm_server_tick(&mut app, 2, Tick(31));
        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(2).map(|(t, v)| (t, v.clone())),
            Some((Tick(31), TestComp(10.0)))
        );
    }

    #[test]
    fn diff_history_waits_when_completed_tick_patch_is_pending() {
        let mut app = setup_app(Tick(5), 40);
        app.add_systems(Update, update_confirmed_history_diff::<TestComp>);
        confirm_server_tick(&mut app, 1, Tick(5));

        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(0), TestComp(0.0));
        let mut receiver = ConfirmedHistoryPatchReceiver::<TestComp>::default();
        receiver.record_cursor(Tick(0), Some(0));
        receiver
            .queue_patches(Tick(5), 4, vec![vec![4.0], vec![5.0]])
            .unwrap();

        let entity = app
            .world_mut()
            .spawn((Interpolated, history, receiver))
            .id();

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(0), TestComp(0.0)))
        );

        let receiver = app
            .world()
            .get::<ConfirmedHistoryPatchReceiver<TestComp>>(entity)
            .unwrap();
        assert!(receiver.has_pending_patches());
        assert_eq!(receiver.tick_for_cursor(Some(0)), Some(Tick(0)));
    }

    #[test]
    fn update_confirmed_history_diff_advances_when_only_older_patch_is_pending() {
        let mut app = setup_app(Tick(6), 40);
        app.add_systems(Update, update_confirmed_history_diff::<TestComp>);
        confirm_server_tick(&mut app, 1, Tick(6));

        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(0), TestComp(0.0));
        let mut receiver = ConfirmedHistoryPatchReceiver::<TestComp>::default();
        receiver.record_cursor(Tick(0), Some(0));
        receiver
            .queue_patches(Tick(5), 4, vec![vec![4.0], vec![5.0]])
            .unwrap();

        let entity = app
            .world_mut()
            .spawn((Interpolated, history, receiver))
            .id();

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(0), TestComp(0.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(6), TestComp(0.0)))
        );

        let receiver = app
            .world()
            .get::<ConfirmedHistoryPatchReceiver<TestComp>>(entity)
            .unwrap();
        assert!(receiver.has_pending_patches());
        assert_eq!(receiver.tick_for_cursor(Some(0)), Some(Tick(0)));
    }

    #[test]
    fn update_confirmed_history_does_not_move_history_backwards() {
        let mut app = setup_app(Tick(30), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);
        confirm_server_tick(&mut app, 1, Tick(100));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(120), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(120), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_advances_from_server_mutate_ticks_without_entity_confirm_history() {
        let mut app = setup_app(Tick(30), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);
        confirm_server_tick(&mut app, 1, Tick(20));
        confirm_server_tick(&mut app, 2, Tick(30));

        let entity = app.world_mut().spawn(TestComp(9.5)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(10.0)))
        );
    }

    #[test]
    fn update_confirmed_history_keeps_bracketing_pair_during_loss_gap() {
        let mut app = setup_app(Tick(25), 40);
        app.add_systems(
            Update,
            (
                update_confirmed_history::<TestComp>,
                interpolate::<TestComp>,
            )
                .chain(),
        );
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);

        let entity = app.world_mut().spawn(TestComp(999.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        history.insert_present(Tick(30), TestComp(20.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        let component = app.world().get::<TestComp>(entity).unwrap();
        let history = app
            .world()
            .get::<ConfirmedHistory<TestComp>>(entity)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.start_present().map(|(t, v)| (t, v.clone())),
            Some((Tick(20), TestComp(10.0)))
        );
        assert_eq!(
            history.get_nth_present(1).map(|(t, v)| (t, v.clone())),
            Some((Tick(30), TestComp(20.0)))
        );
        assert_eq!(component, &TestComp(15.0));
    }

    #[test]
    fn update_confirmed_history_waits_to_insert_component_until_start_tick() {
        let mut app = setup_app(Tick(9), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn update_confirmed_history_removes_component_until_start_tick() {
        let mut app = setup_app(Tick(9), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);

        let entity = app.world_mut().spawn(TestComp(99.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn update_confirmed_history_inserts_and_interpolates_when_start_tick_is_reached() {
        let mut app = setup_app(Tick(15), 40);
        app.add_systems(
            Update,
            (
                update_confirmed_history::<TestComp>,
                interpolate::<TestComp>,
            )
                .chain(),
        );
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
    }

    #[test]
    fn component_removal_waits_until_interpolation_tick_reaches_remove_tick() {
        let mut app = setup_app(Tick(15), 40);
        app.add_systems(
            Update,
            (
                update_confirmed_history::<TestComp>,
                interpolate::<TestComp>,
            )
                .chain(),
        );
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);

        let entity = app.world_mut().spawn(TestComp(99.0)).id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(10.0));
        history.insert_removed(Tick(20));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(10.0)));

        set_interpolation_tick(&mut app, Tick(20));
        app.update();
        assert!(!app.world().entity(entity).contains::<TestComp>());
    }

    #[test]
    fn component_reinsert_after_removal_waits_until_insert_tick() {
        let mut app = setup_app(Tick(15), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_removed(Tick(10));
        history.insert_present(Tick(20), TestComp(20.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();
        assert!(!app.world().entity(entity).contains::<TestComp>());

        set_interpolation_tick(&mut app, Tick(20));
        app.update();
        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(20.0)));
    }

    #[test]
    fn update_confirmed_history_seeds_component_at_current_interpolation_sample() {
        let mut app = setup_app(Tick(15), 40);
        app.add_systems(Update, update_confirmed_history::<TestComp>);
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        app.world_mut().insert_resource(registry);

        let entity = app.world_mut().spawn_empty().id();
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));
        insert_confirmed_history(&mut app, entity, history);

        app.update();

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(5.0)));
    }

    #[test]
    fn stale_entity_insert_command_after_despawn_does_not_panic() {
        let mut app = App::new();
        let entity = app.world_mut().spawn_empty().id();
        app.add_systems(Update, move |mut commands: Commands| {
            commands.entity(entity).despawn();
            commands.entity(entity).try_insert(TestComp(1.0));
        });

        app.update();

        assert!(app.world().get_entity(entity).is_err());
    }
}
//
//     use std::net::SocketAddr;
//     use std::str::FromStr;
//     use bevy::utils::{Duration, Instant};
//
//     use bevy::log::LogPlugin;
//     use bevy::prelude::*;
//     use bevy::time::TimeUpdateStrategy;
//     use bevy::{DefaultPlugins, MinimalPlugins};
//     use tracing::{debug, info};
//     use tracing_subscriber::fmt::format::FmtSpan;
//
//     use crate::_reexport::*;
//     use crate::prelude::client::*;
//     use crate::prelude::*;
//     use crate::tests::protocol::*;
//     use crate::tests::stepper::{BevyStepper};
//
//     fn setup() -> (BevyStepper, Entity, Entity) {
//         let frame_duration = Duration::from_millis(10);
//         let tick_duration = Duration::from_millis(10);
//         let shared_config = SharedConfig {
//             enable_replication: false,
//             tick: TickConfig::new(tick_duration),
//             ..Default::default()
//         };
//         let link_conditioner = LinkConditionerConfig {
//             incoming_latency: Duration::from_millis(40),
//             incoming_jitter: Duration::from_millis(5),
//             incoming_loss: 0.05,
//         };
//         let sync_config = SyncConfig::default().speedup_factor(1.0);
//         let prediction_config = PredictionConfig::default().disable(true);
//         let interpolation_delay = Duration::from_millis(100);
//         let interpolation_config = InterpolationConfig::default().with_delay(InterpolationDelay {
//             min_delay: interpolation_delay,
//             send_interval_ratio: 0.0,
//         });
//         let mut stepper = BevyStepper::new(
//             shared_config,
//             sync_config,
//             prediction_config,
//             interpolation_config,
//             link_conditioner,
//             frame_duration,
//         );
//         stepper.init();
//
//         // Create a confirmed entity on the server
//         let server_entity = stepper
//             .server_app
//             .world_mut()
//             .spawn((Component1(0.0), ShouldBeInterpolated))
//             .id();
//
//         // Set the latest received server tick
//         let confirmed_tick = stepper.client_app().world_mut().resource_mut::<ClientConnectionManager>()
//             .replication_receiver
//             .remote_entity_map
//             .get_confirmed_tick(confirmed_entity)
//             .unwrap();
//
//         // Tick once
//         stepper.frame_step();
//         let tick = stepper.client_tick();
//         let interpolated = stepper
//             .client_app
//             .world()//             .get::<Confirmed>(confirmed)
//             .unwrap()
//             .interpolated
//             .unwrap();
//
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(confirmed)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//
//         // check that the interpolated entity got spawned
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Interpolated>(interpolated)
//                 .unwrap()
//                 .confirmed_entity,
//             confirmed
//         );
//
//         // check that the component history got created and is empty
//         let history = ConfirmedHistory::<Component1>::new();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<ConfirmedHistory<Component1>>(interpolated)
//                 .unwrap(),
//             &history,
//         );
//         // check that the confirmed component got replicated
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(0.0)
//         );
//         // check that the interpolate status got updated
//         let interpolation_tick = stepper.interpolation_tick();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: None,
//                 end: (tick, Component1(0.0)).into(),
//                 current: interpolation_tick,
//             }
//         );
//         (stepper, confirmed, interpolated)
//     }
//
//     // Test interpolation
//     #[test]
//     fn test_interpolation() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//         let start_tick = stepper.client_tick();
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//
//         // check that the interpolate status got updated (end becomes start)
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: None,
//                 current: Tick(3),
//                 // current: Tick(3) - interpolation_tick_delay,
//             }
//         );
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(2));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 2.0;
//
//         stepper.frame_step();
//         // check that interpolation is working correctly
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(0), Component1(0.0)).into(),
//                 end: (Tick(2), Component1(2.0)).into(),
//                 current: Tick(4),
//                 // current: Tick(4) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(1.0)
//         );
//         stepper.frame_step();
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(2), Component1(2.0)).into(),
//                 end: None,
//                 current: Tick(5),
//                 // current: Tick(5) - interpolation_tick_delay,
//             }
//         );
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<Component1>(interpolated)
//                 .unwrap(),
//             &Component1(2.0)
//         );
//         Ok(())
//     }
//
//     // We are in the situation: S1 < I
//     // where S1 is a confirmed ticks, and I is the interpolated tick
//     // and we receive S1 < S2 < I
//     // Then we should now start interpolating from S2
//     #[test]
//     fn test_received_more_recent_start() -> anyhow::Result<()> {
//         let (mut stepper, confirmed, interpolated) = setup();
//
//         // reach interpolation start tick
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         stepper.frame_step();
//         assert_eq!(stepper.client_tick(), Tick(5));
//
//         // receive server update
//         // stepper
//         //     .client_mut()
//         //     .set_latest_received_server_tick(Tick(1));
//         stepper
//             .client_app
//             .world_mut()
//             .get_entity_mut(confirmed)
//             .unwrap()
//             .get_mut::<Component1>()
//             .unwrap()
//             .0 = 1.0;
//
//         stepper.frame_step();
//         // check the status uses the more recent server update
//         assert_eq!(
//             stepper
//                 .client_app
//                 .world()//                 .get::<InterpolateStatus<Component1>>(interpolated)
//                 .unwrap(),
//             &InterpolateStatus::<Component1> {
//                 start: (Tick(1), Component1(1.0)).into(),
//                 end: None,
//                 current: Tick(6),
//                 // current: Tick(6) - interpolation_tick_delay,
//             }
//         );
//         Ok(())
//     }
// }
