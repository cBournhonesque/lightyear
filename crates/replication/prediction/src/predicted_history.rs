//! Manages the prediction history buffer, which stores past local predicted component states.
//!
//! The prediction history is used to:
//! 1. Compare local predicted values with confirmed values from the server to detect mismatches
//! 2. Rollback to a past local state and replay the simulation

use crate::archetypes::{
    CachedDiffComponent, CachedPredictionComponent, PruneDiffHistoryWorld, SnapToConfirmedWorld,
};
use crate::registry::PredictionRegistry;
use crate::rollback::{CatchUpGated, DeterministicPredicted};
use crate::{Predicted, SyncComponent, manager::PredictionManager};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::ComponentIdFor;
use bevy_ecs::prelude::*;
use bevy_ecs::resource::IsResource;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_reflect::Reflect;
use bevy_replicon::shared::replication::diff::{DiffBuffer, Diffable as RepliconDiffable};
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use bevy_utils::prelude::DebugName;
use core::fmt::{self, Debug, Display};
use core::ops::{Deref, DerefMut};
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::prelude::{ConfirmedHistory, LocalTimeline};
use lightyear_core::tick::Tick;
use lightyear_core::timeline::LocalTimelineShift;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::diff_history::HistoryDiffReceiver;
use lightyear_replication::prelude::{ConfirmHistory, PreSpawned};
use lightyear_replication::registry::ComponentRegistry;
use lightyear_sync::prelude::{InputTimelineConfig, SyncedLocalTimeline};
use lightyear_utils::ecs::get_component_unchecked;
#[allow(unused_imports)]
use tracing::{debug, info, trace};

/// Number of ticks retained before the latest processed confirmed tick when pruning
/// [`HistoryDiffReceiver`].
///
/// Diff messages can arrive out of order and can span from an older base to a
/// newer final state, e.g. `S4 -> S8` after tick 6 has already been processed.
/// Keeping this margin gives late diff messages a chance to find their
/// historical base in [`ConfirmedHistory`] instead of forcing a snapshot.
pub(crate) const DIFF_HISTORY_TICK_MARGIN: u32 = 12;

/// Holds the history of locally predicted component states.
///
/// This stores only local prediction samples. Authoritative samples from the
/// remote are stored separately in [`ConfirmedHistory`].
#[derive(Component, Debug, Reflect)]
pub struct PredictionHistory<C>(HistoryBuffer<C>);

impl<C> Default for PredictionHistory<C> {
    fn default() -> Self {
        Self(HistoryBuffer::default())
    }
}

impl<C> Deref for PredictionHistory<C> {
    type Target = HistoryBuffer<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C> DerefMut for PredictionHistory<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<C: Debug> Display for PredictionHistory<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PredictionHistory[")?;
        for (i, (tick, state)) in self.buffer().iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            let state_char = match state {
                HistoryState::Updated(_) => "P",
                HistoryState::Removed => "R",
            };
            write!(f, "{:?}:{}", tick, state_char)?;
        }
        write!(f, "]")
    }
}

impl<C> PredictionHistory<C> {
    /// Add a predicted value or removal computed locally.
    pub fn add_predicted(&mut self, tick: Tick, value: Option<C>) {
        self.add(tick, value);
    }
}

// ============================================================================
// Systems
// ============================================================================

/// Store every update on the predicted entity in the [`PredictionHistory`].
///
/// [`SyncedLocalTimeline`] skips this system until timeline synchronization has completed, so
/// pre-sync component values are never recorded under invalid local tick labels. This system only
/// handles changes; removals are handled by [`apply_component_removal_predicted`].
pub(crate) fn update_prediction_history<T: Component + Clone>(
    manager: Res<PredictionManager>,
    input_config: Res<InputTimelineConfig>,
    mut query: Query<(Entity, Ref<T>, &mut PredictionHistory<T>)>,
    timeline: SyncedLocalTimeline,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();
    let oldest_rollback_tick = tick
        - u32::from(
            manager
                .rollback_policy
                .effective_max_rollback_ticks(&input_config),
        );

    // Update history if the predicted component changed, then prune it.
    for (entity, component, mut history) in query.iter_mut() {
        // change detection works even when running the schedule for rollback
        if component.is_changed() {
            history.add_predicted(tick, Some(component.deref().clone()));
            // Structured per-entity snapshot — `entity` is included so queries
            // against the JSONL can segment history growth/reset by entity
            // (e.g. to tell a deterministic-only ball's history apart from a
            // just-arrived replicated player's history).
            trace!(
                target: "lightyear_debug::prediction",
                kind = "prediction_history_predicted",
                schedule = "FixedPostUpdate",
                sample_point = "FixedPostUpdate",
                entity = ?entity,
                component = ?DebugName::type_name::<T>(),
                local_tick = tick.0,
                history_len = history.len(),
                "recorded predicted component history"
            );
        }
        history.clear_until_tick(oldest_rollback_tick);
    }
}

/// Prune authoritative history on predicted entities to the rollback horizon.
///
/// Runs once per frame in `PostUpdate`: after the fixed loop has recorded history
/// and after the rollback check for the current frame, so snapshots received in
/// `PreUpdate` remain visible to the rollback decision, including forced
/// catch-up rollbacks.
pub(crate) fn prune_confirmed_history<T: Component + Clone>(
    manager: Res<PredictionManager>,
    input_config: Res<InputTimelineConfig>,
    mut query: Query<&mut ConfirmedHistory<T>, With<PredictionHistory<T>>>,
    timeline: SyncedLocalTimeline,
) {
    let oldest_rollback_tick = timeline.tick()
        - u32::from(
            manager
                .rollback_policy
                .effective_max_rollback_ticks(&input_config),
        );

    for mut history in query.iter_mut() {
        history.clear_until_tick(oldest_rollback_tick);
    }
}

/// Shift locally indexed prediction history when the local simulation clock jumps.
pub(crate) fn handle_local_timeline_shift_prediction_history<C: Component>(
    trigger: On<LocalTimelineShift>,
    mut query: Query<&mut PredictionHistory<C>>,
) {
    for mut history in query.iter_mut() {
        history.update_ticks(trigger.delta);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "prediction_history_tick_delta",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            component = ?DebugName::type_name::<C>(),
            tick_delta = trigger.delta,
            history_len = history.len(),
            "shifted prediction history ticks"
        );
    }
}

pub(crate) fn handle_local_timeline_shift_history_diff_receiver<C: RepliconDiffable>(
    trigger: On<LocalTimelineShift>,
    mut storage: ResMut<ReplicationStorage>,
) {
    for (entity, entity_storage) in storage.entities.iter_mut() {
        let Some(receiver) = entity_storage.get_mut::<HistoryDiffReceiver<C>>() else {
            continue;
        };
        receiver.update_ticks(trigger.delta);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_diff_receiver_tick_delta",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?entity,
            component = ?DebugName::type_name::<C>(),
            tick_delta = trigger.delta,
            "shifted confirmed history diff receiver ticks"
        );
    }
}

/// Prune historical diff cursor state that is no longer needed for rollback.
///
/// This promotes the newest cursor at or before `last_processed_confirmed_tick -
/// DIFF_HISTORY_TICK_MARGIN` to the receiver's retained base. The margin keeps
/// older confirmed values available for late diff messages whose base is
/// before the latest processed tick but whose target tick has not been received
/// yet.
pub(crate) fn prune_history_diff_receiver(
    mut prediction_world: PruneDiffHistoryWorld,
    state_metadata: Res<crate::manager::StateRollbackMetadata>,
    mut storage: ResMut<ReplicationStorage>,
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
) {
    let Some(last_processed_confirmed_tick) = state_metadata.last_processed_confirmed_tick() else {
        return;
    };
    let prune_tick = last_processed_confirmed_tick - DIFF_HISTORY_TICK_MARGIN;

    prediction_world.update_archetypes(&prediction_registry, &component_registry);
    let world = prediction_world.world;
    for (archetype, cached) in prediction_world.diff_archetypes() {
        if !cached.default_query_target {
            continue;
        }
        for component in &cached.diff_components {
            if component.confirmed_history_storage.is_none() {
                continue;
            }
            // SAFETY: the cache records the exact confirmed-history column, and the system
            // declares read access to it while holding unique access to ReplicationStorage.
            unsafe {
                (component.prune_history_diff_receiver)(
                    world,
                    archetype,
                    component,
                    prune_tick,
                    storage.as_mut(),
                );
            }
        }
    }
}

/// Prunes one cached diff receiver type across an archetype.
///
/// # Safety
///
/// `component` must describe `ConfirmedHistory<C>` for `archetype`, and the caller must have read
/// access to that column and unique access to `storage`.
pub(crate) unsafe fn prune_history_diff_receiver_component<C: RepliconDiffable>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedDiffComponent,
    prune_tick: Tick,
    storage: &mut ReplicationStorage,
) {
    let confirmed_history_storage = component
        .confirmed_history_storage
        .expect("diff pruning requires confirmed history");
    for entity in archetype.entities() {
        let history = unsafe {
            get_component_unchecked(
                world,
                entity,
                archetype.table_id(),
                confirmed_history_storage,
                component.confirmed_history_id,
            )
            .deref::<ConfirmedHistory<C>>()
        };
        let Some(receiver) = storage.get_mut::<HistoryDiffReceiver<C>>(entity.id()) else {
            continue;
        };
        if !receiver.has_pending_diffs() {
            // The receiver's retained base follows the oldest value retained in
            // `ConfirmedHistory`. With no value at the retained floor (i.e. the
            // component is removed there), no cursor is promoted.
            let base_is_present = history
                .get_state_at_or_before(prune_tick)
                .and_then(HistoryState::value)
                .is_some();
            receiver.clear_before_tick(prune_tick, base_is_present);
        }
    }
}
/// If a predicted component is removed on the [`Predicted`] entity, add the removal to the history.
/// [`SyncedLocalTimeline`] skips this observer before timeline synchronization has completed.
pub(crate) fn apply_component_removal_predicted<C: Component>(
    trigger: On<Remove, C>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
    timeline: SyncedLocalTimeline,
) {
    let tick = timeline.tick();
    if let Ok(mut history) = predicted_query.get_mut(trigger.entity) {
        history.add_predicted(tick, None);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "prediction_history_removed",
            schedule = "FixedPostUpdate",
            sample_point = "FixedPostUpdate",
            entity = ?trigger.entity,
            component = ?DebugName::type_name::<C>(),
            local_tick = tick.0,
            history_len = history.len(),
            "recorded predicted component removal"
        );
    }
}

/// When `C` or one of [`Predicted`], [`PreSpawned`], [`DeterministicPredicted`], or
/// [`CatchUpGated`] is added to an entity, ensure [`PredictionHistory<C>`] is present for predicted
/// entities and resource entities. [`IsResource`] is an eligibility check rather than a trigger:
/// `Add<C>` already fires when a resource is inserted, and triggering on every `IsResource`
/// addition would wake this observer for unrelated resource types.
///
/// [`CatchUpGated`] always needs [`PredictionHistory<C>`], even while `C` is absent because its
/// replicated value is still gated in [`ConfirmedHistory<C>`]. Prediction history is also the
/// component's rollback-membership marker, so it must be present for the catch-up rollback to
/// materialize that confirmed value as the live component.
pub(crate) fn add_prediction_history<C: Component + Clone>(
    trigger: On<
        Add,
        (
            C,
            Predicted,
            PreSpawned,
            DeterministicPredicted,
            CatchUpGated,
        ),
    >,
    query: Query<
        (),
        Or<(
            With<CatchUpGated>,
            (
                With<C>,
                Or<(
                    With<Predicted>,
                    With<PreSpawned>,
                    With<DeterministicPredicted>,
                    With<IsResource>,
                )>,
            ),
        )>,
    >,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_err() {
        return;
    }
    trace!(
        target: "lightyear_debug::prediction",
        kind = "prediction_history_insert",
        entity = ?trigger.entity,
        component = ?DebugName::type_name::<C>(),
        "inserted prediction history component"
    );
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            return;
        };
        entity_mut.insert_if_new(PredictionHistory::<C>::default());
    });
}

/// Seeds the last replicated value when [`Predicted`] is added manually to an
/// already-replicated entity on the receiver.
///
/// Initial replication through `PredictedSend` also adds the receiver-side [`Predicted`] marker,
/// but its marker writer adds [`ConfirmedHistory<C>`] in the same replication command. The
/// [`Without<ConfirmedHistory<C>>`] query filter therefore makes that path a no-op here.
///
/// Without this baseline an unchanged authoritative component might never
/// receive another update, leaving state-based prediction unable to compare it
/// at later completed server ticks.
pub(crate) fn backfill_confirmed_history_on_predicted<C: SyncComponent>(
    trigger: On<Add, Predicted>,
    query: Query<(&C, &ConfirmHistory), Without<ConfirmedHistory<C>>>,
    component_id: ComponentIdFor<C>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    mut commands: Commands,
) {
    // A component added together with Predicted is a new local value, not a
    // previously replicated authoritative baseline.
    if trigger.trigger().components.contains(&component_id.get()) {
        return;
    }
    let Ok((component, confirm_history)) = query.get(trigger.entity) else {
        return;
    };
    let Some(tick) = checkpoints
        .as_deref()
        .and_then(|checkpoints| checkpoints.get(confirm_history.last_tick()))
    else {
        return;
    };
    let mut history = ConfirmedHistory::<C>::default();
    history.insert_present_explicit(tick, component.clone());
    trace!(
        entity = ?trigger.entity,
        ?tick,
        component = ?DebugName::type_name::<C>(),
        "backfilled confirmed history for late prediction opt-in"
    );
    commands.entity(trigger.entity).insert(history);
}

pub(crate) fn add_history_diff_receiver<C: SyncComponent + RepliconDiffable>(
    trigger: On<Add, (C, Predicted, PreSpawned, DeterministicPredicted)>,
    query: Query<
        (),
        (
            With<C>,
            Or<(
                With<Predicted>,
                With<PreSpawned>,
                With<DeterministicPredicted>,
            )>,
        ),
    >,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_err() {
        return;
    }
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        let seed_inputs = {
            let Ok(entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            entity_mut
                .get::<lightyear_replication::prelude::ConfirmHistory>()
                .map(lightyear_replication::prelude::ConfirmHistory::last_tick)
        };
        let seed = seed_inputs.and_then(|confirm_tick| {
            let cursor = world
                .get_resource::<ReplicationStorage>()
                .and_then(|storage| storage.get::<DiffBuffer<C>>(entity))
                .and_then(DiffBuffer::<C>::last_applied)?;
            world
                .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
                .get(confirm_tick)
                .map(|tick| (tick, cursor))
        });
        let Some((tick, cursor)) = seed else {
            return;
        };
        let Some(mut storage) = world.get_resource_mut::<ReplicationStorage>() else {
            return;
        };
        storage.get_or_init::<HistoryDiffReceiver<C>>(entity, || {
            let mut receiver = HistoryDiffReceiver::<C>::default();
            receiver.record_cursor(tick, Some(cursor));
            receiver
        });
    });
}

/// During rollback re-simulation, check if we have a confirmed value for this tick.
/// If so, snap the component to the confirmed value instead of using the predicted value.
///
/// [`PredictionSystems::SnapToConfirmed`](crate::plugin::PredictionSystems::SnapToConfirmed) gates
/// this system with [`is_in_rollback`](lightyear_core::timeline::is_in_rollback), so the global
/// [`Rollback`](lightyear_core::timeline::Rollback) resource does not need to be fetched by every
/// monomorphized component system.
pub(crate) fn snap_to_confirmed_during_rollback(
    mut prediction_world: SnapToConfirmedWorld,
    timeline: Option<Res<LocalTimeline>>,
    prediction_registry: Res<PredictionRegistry>,
    component_registry: Res<ComponentRegistry>,
    mut commands: Commands,
) {
    let Some(timeline) = timeline else {
        return;
    };
    let tick = timeline.tick();
    prediction_world.update_archetypes(&prediction_registry, &component_registry);

    let world = prediction_world.world;
    let mut deferred = DeferredEntityCommands::default();
    for (archetype, cached) in prediction_world.predicted_archetypes() {
        if !cached.default_query_target || !cached.contains_predicted_marker {
            continue;
        }
        for component in &cached.predicted_components {
            let (Some(snap_to_confirmed), Some(_)) = (
                component.snap_to_confirmed,
                component.confirmed_history_storage,
            ) else {
                continue;
            };
            // SAFETY: the cache records the exact live and confirmed-history columns, and the
            // system declares write access to the former and read access to the latter.
            unsafe {
                snap_to_confirmed(world, archetype, component, tick, &mut deferred);
            }
        }
    }
    deferred.apply(&mut commands);
}

/// Snaps one cached predicted component type across an archetype during rollback replay.
///
/// # Safety
///
/// `component` must describe `C` and `ConfirmedHistory<C>` for `archetype`, and the caller must
/// hold the accesses declared by [`SnapToConfirmedWorld`].
pub(crate) unsafe fn snap_to_confirmed_component<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedPredictionComponent,
    tick: Tick,
    deferred: &mut DeferredEntityCommands,
) {
    let confirmed_history_storage = component
        .confirmed_history_storage
        .expect("replay snap requires confirmed history");
    for entity in archetype.entities() {
        let entity_id = entity.id();
        let history = unsafe {
            get_component_unchecked(
                world,
                entity,
                archetype.table_id(),
                confirmed_history_storage,
                component.confirmed_history_id,
            )
            .deref::<ConfirmedHistory<C>>()
        };
        let live_component = component.component_storage.map(|storage| unsafe {
            get_component_unchecked(
                world,
                entity,
                archetype.table_id(),
                storage,
                component.component_id,
            )
            .deref::<C>()
        });

        // Check if there's a confirmed value at exactly this tick
        if let Some(confirmed_state) = history.get_state_at(tick) {
            match confirmed_state {
                HistoryState::Updated(confirmed_value) => {
                    // Snap to the confirmed value
                    if let Some(current) = live_component {
                        if current != confirmed_value {
                            trace!(
                                target: "lightyear_debug::prediction",
                                kind = "snap_to_confirmed",
                                schedule = "FixedPreUpdate",
                                sample_point = "FixedPreUpdate",
                                entity = ?entity_id,
                                component = ?DebugName::type_name::<C>(),
                                local_tick = tick.0,
                                confirmed_tick = tick.0,
                                value = ?confirmed_value,
                                "snapped predicted component to confirmed value during rollback"
                            );
                            // SAFETY: the dispatcher declares unique access to C, and no live-C
                            // reference is used after this call.
                            unsafe {
                                lightyear_core::ecs_utils::write_component_with_change_detection::<C>(
                                    world,
                                    entity_id,
                                    confirmed_value.clone(),
                                );
                            }
                        }
                    } else {
                        // Component doesn't exist but should - insert it
                        debug!(
                            ?entity_id,
                            ?tick,
                            "Inserting confirmed component during rollback for {:?}",
                            DebugName::type_name::<C>()
                        );
                        trace!(
                            target: "lightyear_debug::prediction",
                            kind = "snap_to_confirmed_insert",
                            schedule = "FixedPreUpdate",
                            sample_point = "FixedPreUpdate",
                            entity = ?entity_id,
                            component = ?DebugName::type_name::<C>(),
                            local_tick = tick.0,
                            confirmed_tick = tick.0,
                            value = ?confirmed_value,
                            "inserted confirmed component during rollback"
                        );
                        deferred.insert(entity_id, confirmed_value.clone());
                    }
                }
                HistoryState::Removed => {
                    // Confirmed removal - remove the component if it exists
                    if live_component.is_some() {
                        debug!(
                            ?entity_id,
                            ?tick,
                            "Removing component during rollback (confirmed removal) for {:?}",
                            DebugName::type_name::<C>()
                        );
                        trace!(
                            target: "lightyear_debug::prediction",
                            kind = "snap_to_confirmed_remove",
                            schedule = "FixedPreUpdate",
                            sample_point = "FixedPreUpdate",
                            entity = ?entity_id,
                            component = ?DebugName::type_name::<C>(),
                            local_tick = tick.0,
                            confirmed_tick = tick.0,
                            "removed component for confirmed removal during rollback"
                        );
                        deferred.remove::<C>(entity_id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{RollbackPolicy, StateRollbackMetadata};
    use bevy_app::{App, Update};
    use bevy_ecs::entity_disabling::Disabled;
    use bevy_ecs::system::RunSystemOnce;
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use lightyear_core::prelude::FrameInterpolationHistory;
    use lightyear_sync::prelude::LocalTimelineSync;
    use lightyear_sync::timeline::input::InputDelayConfig;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, PartialEq, Debug)]
    struct TestValue(f32);

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestDiffValue(u32);

    impl RepliconDiffable for TestDiffValue {
        type Diff = u32;

        fn apply_diff(&mut self, diff: &Self::Diff) -> bevy_ecs::error::Result<()> {
            self.0 = *diff;
            Ok(())
        }
    }

    fn idx(value: u16) -> DiffIndex {
        DiffIndex::new(value)
    }

    #[test]
    fn test_clear_after_tick_removes_newer_predictions() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_predicted(Tick(1), Some(TestValue(1.0)));
        history.add_predicted(Tick(5), Some(TestValue(5.0)));
        history.add_predicted(Tick(9), Some(TestValue(9.0)));

        let restore_value = history.get(Tick(4)).cloned();
        history.clear_after_tick(Tick(4));

        assert!(matches!(restore_value, Some(TestValue(v)) if v == 1.0));

        let has_tick_5 = history.buffer().iter().any(|(t, _)| *t == Tick(5));
        let has_tick_9 = history.buffer().iter().any(|(t, _)| *t == Tick(9));
        assert!(!has_tick_5);
        assert!(!has_tick_9);
    }

    fn prediction_history_test_app(
        max_rollback_ticks: u16,
        input_delay_config: InputDelayConfig,
        tick: i32,
    ) -> App {
        let mut app = App::new();
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(tick);
        app.insert_resource(timeline);
        app.insert_resource(PredictionManager {
            rollback_policy: RollbackPolicy {
                max_rollback_ticks,
                ..Default::default()
            },
            ..Default::default()
        });
        app.insert_resource(InputTimelineConfig::default().with_input_delay(input_delay_config));
        let mut sync = LocalTimelineSync::default();
        sync.set_synced(true);
        app.insert_resource(sync);
        app
    }

    #[test]
    fn prediction_history_is_not_recorded_until_timeline_sync() {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<LocalTimelineSync>();
        app.insert_resource(PredictionManager::default());
        app.insert_resource(InputTimelineConfig::default());
        app.add_systems(Update, update_prediction_history::<TestValue>);
        app.add_observer(apply_component_removal_predicted::<TestValue>);

        let entity = app
            .world_mut()
            .spawn((TestValue(1.0), PredictionHistory::<TestValue>::default()))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<PredictionHistory<TestValue>>(entity)
                .unwrap()
                .is_empty(),
            "an unsynchronized local tick must not label a prediction sample"
        );

        app.world_mut().entity_mut(entity).remove::<TestValue>();
        assert!(
            app.world()
                .get::<PredictionHistory<TestValue>>(entity)
                .unwrap()
                .is_empty(),
            "an unsynchronized local tick must not label a removal sample"
        );

        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        app.world_mut().entity_mut(entity).insert(TestValue(2.0));
        app.update();

        let history = app
            .world()
            .get::<PredictionHistory<TestValue>>(entity)
            .unwrap();
        assert_eq!(history.get(Tick(0)), Some(&TestValue(2.0)));
    }

    #[test]
    fn prediction_history_is_pruned_to_effective_rollback_horizon() {
        let mut app = prediction_history_test_app(20, InputDelayConfig::balanced(), 100);
        app.add_systems(
            Update,
            (
                update_prediction_history::<TestValue>,
                prune_confirmed_history::<TestValue>,
            ),
        );

        let mut history = PredictionHistory::default();
        for tick in [80, 90, 95, 100] {
            history.add_predicted(Tick(tick), Some(TestValue(tick as f32)));
        }
        let mut confirmed_history = ConfirmedHistory::default();
        for tick in [80, 90, 95, 100] {
            confirmed_history.insert_present(Tick(tick), TestValue(tick as f32));
        }
        let entity = app
            .world_mut()
            .spawn((TestValue(100.0), history, confirmed_history))
            .id();

        app.update();

        let history = app
            .world()
            .get::<PredictionHistory<TestValue>>(entity)
            .unwrap();
        assert_eq!(history.oldest().unwrap().0, Tick(93));
        assert_eq!(
            history.get(Tick(93)),
            Some(&TestValue(90.0)),
            "balanced input delay should cap the 20-tick policy at 7 ticks"
        );
        let confirmed_history = app
            .world()
            .get::<ConfirmedHistory<TestValue>>(entity)
            .unwrap();
        assert_eq!(confirmed_history.len(), 3);
    }

    #[test]
    fn diff_receiver_pruning_keeps_margin_before_last_processed_confirmed_tick() {
        let mut app = App::new();
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_confirmed_tick(Tick(16));
        app.insert_resource(metadata);
        app.insert_resource(ReplicationStorage::default());
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<ComponentRegistry>();
        app.world_mut().register_component::<TestDiffValue>();
        let prediction_history_id = app
            .world_mut()
            .register_component::<PredictionHistory<TestDiffValue>>();
        let confirmed_history_id = app
            .world_mut()
            .register_component::<ConfirmedHistory<TestDiffValue>>();
        let frame_interpolation_history_id = app
            .world_mut()
            .register_component::<FrameInterpolationHistory<TestDiffValue>>();
        let mut registry = app.world_mut().resource_mut::<PredictionRegistry>();
        registry.register_rollback::<TestDiffValue>(
            prediction_history_id,
            confirmed_history_id,
            frame_interpolation_history_id,
        );
        registry.register::<TestDiffValue>();
        registry.set_pending_diff_tick::<TestDiffValue>();
        drop(registry);
        app.add_systems(Update, prune_history_diff_receiver);

        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(2), TestDiffValue(2));
        history.insert_present(Tick(4), TestDiffValue(4));
        history.insert_present(Tick(8), TestDiffValue(8));

        let mut receiver = HistoryDiffReceiver::<TestDiffValue>::default();
        receiver.record_cursor(Tick(2), Some(idx(2)));
        receiver.record_cursor(Tick(4), Some(idx(4)));
        receiver.record_cursor(Tick(8), Some(idx(8)));

        let entity = app.world_mut().spawn(history).id();
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .insert(entity, receiver);
        app.update();

        let receiver = app
            .world()
            .resource::<ReplicationStorage>()
            .get::<HistoryDiffReceiver<TestDiffValue>>(entity)
            .unwrap();
        assert_eq!(receiver.tick_for_cursor(Some(idx(2))), None);
        assert_eq!(receiver.tick_for_cursor(Some(idx(4))), Some(Tick(4)));
        assert_eq!(receiver.tick_for_cursor(Some(idx(8))), Some(Tick(8)));
    }

    #[test]
    fn snap_to_confirmed_matches_typed_query_behavior() {
        const TICK: Tick = Tick(10);

        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<ComponentRegistry>();
        let prediction_history_id = app
            .world_mut()
            .register_component::<PredictionHistory<TestValue>>();
        let confirmed_history_id = app
            .world_mut()
            .register_component::<ConfirmedHistory<TestValue>>();
        let frame_interpolation_history_id = app
            .world_mut()
            .register_component::<FrameInterpolationHistory<TestValue>>();
        let mut registry = app.world_mut().resource_mut::<PredictionRegistry>();
        registry.register_rollback::<TestValue>(
            prediction_history_id,
            confirmed_history_id,
            frame_interpolation_history_id,
        );
        registry.register::<TestValue>();
        registry.set_snap_to_confirmed::<TestValue>();
        drop(registry);

        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(TICK.0 as i32);
        app.insert_resource(timeline);

        let present_history = |value| {
            let mut history = ConfirmedHistory::default();
            history.insert_present(TICK, TestValue(value));
            history
        };

        let changed = app
            .world_mut()
            .spawn((Predicted, TestValue(1.0), present_history(2.0)))
            .id();
        let unchanged = app
            .world_mut()
            .spawn((Predicted, TestValue(3.0), present_history(3.0)))
            .id();
        let inserted = app
            .world_mut()
            .spawn((Predicted, present_history(4.0)))
            .id();

        let mut removed_history = ConfirmedHistory::<TestValue>::default();
        removed_history.insert_removed(TICK);
        let removed = app
            .world_mut()
            .spawn((Predicted, TestValue(5.0), removed_history))
            .id();

        // The old Query obeyed Bevy's default disabling filters. Cached
        // archetypes must exclude the same entities.
        let disabled = app
            .world_mut()
            .spawn((Predicted, Disabled, TestValue(6.0), present_history(7.0)))
            .id();

        app.world_mut().clear_trackers();
        let changed_tick = app
            .world()
            .entity(changed)
            .get_change_ticks::<TestValue>()
            .unwrap()
            .changed;
        let unchanged_tick = app
            .world()
            .entity(unchanged)
            .get_change_ticks::<TestValue>()
            .unwrap()
            .changed;

        app.world_mut()
            .run_system_once(snap_to_confirmed_during_rollback)
            .unwrap();

        assert_eq!(app.world().get::<TestValue>(changed), Some(&TestValue(2.0)));
        assert_ne!(
            app.world()
                .entity(changed)
                .get_change_ticks::<TestValue>()
                .unwrap()
                .changed,
            changed_tick,
            "a changed live value must retain Bevy change detection"
        );
        assert_eq!(
            app.world().get::<TestValue>(unchanged),
            Some(&TestValue(3.0))
        );
        assert_eq!(
            app.world()
                .entity(unchanged)
                .get_change_ticks::<TestValue>()
                .unwrap()
                .changed,
            unchanged_tick,
            "an equal confirmed value must not spuriously mark the component changed"
        );
        assert_eq!(
            app.world().get::<TestValue>(inserted),
            Some(&TestValue(4.0))
        );
        assert!(app.world().get::<TestValue>(removed).is_none());
        assert_eq!(
            app.world().get::<TestValue>(disabled),
            Some(&TestValue(6.0)),
            "default-disabled entities must stay outside the cached query universe"
        );
    }
}
