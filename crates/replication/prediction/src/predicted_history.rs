//! Manages the prediction history buffer, which stores past local predicted component states.
//!
//! The prediction history is used to:
//! 1. Compare local predicted values with confirmed values from the server to detect mismatches
//! 2. Rollback to a past local state and replay the simulation

use crate::rollback::DeterministicPredicted;
use crate::{Predicted, SyncComponent};
use bevy_ecs::component::Mutable;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::shared::replication::diff::{Diffable as RepliconDiffable, PatchBuffer};
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use bevy_utils::prelude::DebugName;
use core::fmt::{self, Debug, Display};
use core::ops::{Deref, DerefMut};
use lightyear_core::history_buffer::{HistoryBuffer, HistoryState};
use lightyear_core::prelude::{ConfirmedHistory, LocalTimeline};
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{Rollback, SyncEvent};
use lightyear_replication::diff_history::ConfirmedHistoryPatchReceiver;
use lightyear_replication::prelude::PreSpawned;
use lightyear_sync::prelude::InputTimelineConfig;
#[allow(unused_imports)]
use tracing::{debug, info, trace};

/// Number of ticks retained before the latest processed confirmed tick when pruning
/// [`ConfirmedHistoryPatchReceiver`].
///
/// Diff messages can arrive out of order and can span from an older base to a
/// newer final state, e.g. `S4 -> S8` after tick 6 has already been processed.
/// Keeping this margin gives late patch messages a chance to find their
/// historical base in [`ConfirmedHistory`] instead of forcing a snapshot.
pub(crate) const PATCH_HISTORY_TICK_MARGIN: u32 = 12;

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

/// We store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + Clone + Debug>(
    mut query: Query<(Entity, Ref<T>, &mut PredictionHistory<T>)>,
    timeline: Res<LocalTimeline>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();

    // update history if the predicted component changed
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
                value = ?component.deref(),
                "recorded predicted component history"
            );
        }
    }
}

/// If there is a TickEvent and the client tick suddenly changes, we need
/// to update the ticks in the history buffer.
pub(crate) fn handle_tick_event_prediction_history<C: Component>(
    trigger: On<SyncEvent<InputTimelineConfig>>,
    mut query: Query<&mut PredictionHistory<C>>,
) {
    for mut history in query.iter_mut() {
        history.update_ticks(trigger.tick_delta);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "prediction_history_tick_delta",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?trigger.entity,
            component = ?DebugName::type_name::<C>(),
            tick_delta = trigger.tick_delta,
            history_len = history.len(),
            "shifted prediction history ticks"
        );
    }
}

/// If there is a TickEvent and the client tick suddenly changes, update confirmed-history ticks too.
pub(crate) fn handle_tick_event_confirmed_history<C: Component>(
    trigger: On<SyncEvent<InputTimelineConfig>>,
    mut query: Query<&mut ConfirmedHistory<C>>,
) {
    for mut history in query.iter_mut() {
        history.update_ticks(trigger.tick_delta);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_tick_delta",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?trigger.entity,
            component = ?DebugName::type_name::<C>(),
            tick_delta = trigger.tick_delta,
            history_len = history.len(),
            "shifted confirmed history ticks"
        );
    }
}

pub(crate) fn handle_tick_event_confirmed_history_patch_receiver<C: RepliconDiffable>(
    trigger: On<SyncEvent<InputTimelineConfig>>,
    mut storage: ResMut<ReplicationStorage>,
) {
    for (entity, entity_storage) in storage.entities.iter_mut() {
        let Some(receiver) = entity_storage.get_mut::<ConfirmedHistoryPatchReceiver<C>>() else {
            continue;
        };
        receiver.update_ticks(trigger.tick_delta);
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_patch_receiver_tick_delta",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?entity,
            component = ?DebugName::type_name::<C>(),
            tick_delta = trigger.tick_delta,
            "shifted confirmed history patch receiver ticks"
        );
    }
}

/// Prune historical patch cursor state that is no longer needed for rollback.
///
/// This promotes the newest cursor at or before `last_processed_tick -
/// PATCH_HISTORY_TICK_MARGIN` to the receiver's retained base. The margin keeps
/// older confirmed values available for late patch messages whose base is
/// before the latest processed tick but whose target tick has not been received
/// yet.
pub(crate) fn prune_confirmed_history_patch_receiver<C: RepliconDiffable>(
    state_metadata: Res<crate::manager::StateRollbackMetadata>,
    mut storage: ResMut<ReplicationStorage>,
    query: Query<(Entity, &ConfirmedHistory<C>)>,
) {
    let Some(last_processed_tick) = state_metadata.last_processed_tick() else {
        return;
    };
    let prune_tick = last_processed_tick - PATCH_HISTORY_TICK_MARGIN;
    for (entity, history) in query.iter() {
        let Some(receiver) = storage.get_mut::<ConfirmedHistoryPatchReceiver<C>>(entity) else {
            continue;
        };
        if !receiver.has_pending_patches() {
            receiver.clear_before_tick(prune_tick, history);
        }
    }
}

/// If a predicted component is removed on the [`Predicted`] entity, add the removal to the history.
pub(crate) fn apply_component_removal_predicted<C: Component>(
    trigger: On<Remove, C>,
    mut predicted_query: Query<&mut PredictionHistory<C>>,
    timeline: Res<LocalTimeline>,
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

/// When any of `C`, [`Predicted`], [`PreSpawned`], or [`DeterministicPredicted`]
/// is added to an entity, ensure [`PredictionHistory<C>`] is present, and if
/// `C` has just been applied via an init message, seed [`ConfirmedHistory<C>`]
/// at the server tick that produced the init.
///
/// # Why seeding is needed
///
/// Replicon reads entity markers on the empty newly-spawned entity BEFORE
/// init components are applied. As a result, the marker-gated `write_history`
/// function does NOT fire for init messages — the component value is written
/// directly to the entity via the default write, and `ConfirmedHistory<C>` gets
/// no confirmed entry for the init tick. We plug that hole here.
///
/// # Once-only semantics
///
/// Seeding only happens when confirmed history does not already exist. We must
/// not overwrite existing local prediction history or existing authoritative
/// samples. If there is no authoritative seed, no confirmed history is inserted:
/// receive paths create it when a confirmed sample actually arrives.
pub(crate) fn add_prediction_history<C: SyncComponent>(
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
    trace!(
        target: "lightyear_debug::prediction",
        kind = "prediction_history_insert",
        entity = ?trigger.entity,
        component = ?DebugName::type_name::<C>(),
        "inserted prediction history component"
    );
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        let Ok(entity_mut) = world.get_entity_mut(entity) else {
            return;
        };
        let has_prediction_history = entity_mut.contains::<PredictionHistory<C>>();
        let has_confirmed_history = entity_mut.contains::<ConfirmedHistory<C>>();
        if has_prediction_history && has_confirmed_history {
            return;
        }
        // Try to capture a confirmed entry from the current C value + the
        // server tick resolved via ConfirmHistory. This path only fires
        // when all of `C`, `ConfirmHistory`, and a checkpoint mapping
        // are present — i.e. when this is an init-message write.
        let seed: Option<(Tick, C)> = {
            let component = entity_mut.get::<C>().cloned();
            let confirm_last = entity_mut
                .get::<lightyear_replication::prelude::ConfirmHistory>()
                .map(lightyear_replication::prelude::ConfirmHistory::last_tick);
            match (component, confirm_last) {
                (Some(component), Some(confirm_tick)) => world
                    .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
                    .get(confirm_tick)
                    .map(|tick| (tick, component)),
                _ => None,
            }
        };
        // Re-fetch the entity after the world-level resource access above.
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            return;
        };
        if !has_prediction_history {
            entity_mut.insert(PredictionHistory::<C>::default());
        }
        if has_confirmed_history {
            return;
        }
        if let Some((tick, component)) = seed {
            let mut history = ConfirmedHistory::<C>::default();
            trace!(
                ?entity,
                ?tick,
                component = ?DebugName::type_name::<C>(),
                "seeding ConfirmedHistory with confirmed value from init message"
            );
            history.insert_present(tick, component);
            entity_mut.insert(history);
        }
    });
}

pub(crate) fn add_confirmed_history_patch_receiver<C: SyncComponent + RepliconDiffable>(
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
            let confirm_last = entity_mut
                .get::<lightyear_replication::prelude::ConfirmHistory>()
                .map(lightyear_replication::prelude::ConfirmHistory::last_tick);
            confirm_last
        };
        let seed = seed_inputs.and_then(|confirm_tick| {
            let cursor = world
                .get_resource::<ReplicationStorage>()
                .and_then(|storage| storage.get::<PatchBuffer<C>>(entity))
                .and_then(PatchBuffer::<C>::last_applied)?;
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
        storage.get_or_init::<ConfirmedHistoryPatchReceiver<C>>(entity, || {
            let mut receiver = ConfirmedHistoryPatchReceiver::<C>::default();
            receiver.record_cursor(tick, Some(cursor));
            receiver
        });
    });
}

/// During rollback re-simulation, check if we have a confirmed value for this tick.
/// If so, snap the component to the confirmed value instead of using the predicted value.
pub(crate) fn snap_to_confirmed_during_rollback<
    C: Component<Mutability = Mutable> + Clone + PartialEq + Debug,
>(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    // Only run during rollback
    rollback: Single<&Rollback>,
    mut query: Query<(Entity, Option<&mut C>, &ConfirmedHistory<C>), With<Predicted>>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|(entity, component, history)| {
        // Check if there's a confirmed value at exactly this tick
        if let Some(confirmed_state) = history.get_state_at(tick) {
            match confirmed_state {
                HistoryState::Updated(confirmed_value) => {
                    // Snap to the confirmed value
                    if let Some(mut comp) = component {
                        if comp.deref() != confirmed_value {
                            trace!(
                                target: "lightyear_debug::prediction",
                                kind = "snap_to_confirmed",
                                schedule = "FixedPreUpdate",
                                sample_point = "FixedPreUpdate",
                                entity = ?entity,
                                component = ?DebugName::type_name::<C>(),
                                local_tick = tick.0,
                                confirmed_tick = tick.0,
                                value = ?confirmed_value,
                                "snapped predicted component to confirmed value during rollback"
                            );
                            *comp = confirmed_value.clone();
                        }
                    } else {
                        // Component doesn't exist but should - insert it
                        debug!(
                            ?entity,
                            ?tick,
                            "Inserting confirmed component during rollback for {:?}",
                            DebugName::type_name::<C>()
                        );
                        trace!(
                            target: "lightyear_debug::prediction",
                            kind = "snap_to_confirmed_insert",
                            schedule = "FixedPreUpdate",
                            sample_point = "FixedPreUpdate",
                            entity = ?entity,
                            component = ?DebugName::type_name::<C>(),
                            local_tick = tick.0,
                            confirmed_tick = tick.0,
                            value = ?confirmed_value,
                            "inserted confirmed component during rollback"
                        );
                        commands.entity(entity).insert(confirmed_value.clone());
                    }
                }
                HistoryState::Removed => {
                    // Confirmed removal - remove the component if it exists
                    if component.is_some() {
                        debug!(
                            ?entity,
                            ?tick,
                            "Removing component during rollback (confirmed removal) for {:?}",
                            DebugName::type_name::<C>()
                        );
                        trace!(
                            target: "lightyear_debug::prediction",
                            kind = "snap_to_confirmed_remove",
                            schedule = "FixedPreUpdate",
                            sample_point = "FixedPreUpdate",
                            entity = ?entity,
                            component = ?DebugName::type_name::<C>(),
                            local_tick = tick.0,
                            confirmed_tick = tick.0,
                            "removed component for confirmed removal during rollback"
                        );
                        commands.entity(entity).remove::<C>();
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::StateRollbackMetadata;
    use bevy_app::{App, Update};
    use bevy_replicon::shared::replication::diff::patch_index::PatchIndex;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestDiffValue(u32);

    impl RepliconDiffable for TestDiffValue {
        type Patch = u32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    fn idx(value: u16) -> PatchIndex {
        PatchIndex::new(value)
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

    #[test]
    fn patch_receiver_pruning_keeps_margin_before_last_processed_tick() {
        let mut app = App::new();
        let mut metadata = StateRollbackMetadata::default();
        metadata.set_last_processed_tick(Tick(16));
        app.insert_resource(metadata);
        app.insert_resource(ReplicationStorage::default());
        app.add_systems(
            Update,
            prune_confirmed_history_patch_receiver::<TestDiffValue>,
        );

        let mut history = ConfirmedHistory::<TestDiffValue>::default();
        history.insert_present(Tick(2), TestDiffValue(2));
        history.insert_present(Tick(4), TestDiffValue(4));
        history.insert_present(Tick(8), TestDiffValue(8));

        let mut receiver = ConfirmedHistoryPatchReceiver::<TestDiffValue>::default();
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
            .get::<ConfirmedHistoryPatchReceiver<TestDiffValue>>(entity)
            .unwrap();
        assert_eq!(receiver.tick_for_cursor(Some(idx(2))), None);
        assert_eq!(receiver.tick_for_cursor(Some(idx(4))), Some(Tick(4)));
        assert_eq!(receiver.tick_for_cursor(Some(idx(8))), Some(Tick(8)));
    }
}
