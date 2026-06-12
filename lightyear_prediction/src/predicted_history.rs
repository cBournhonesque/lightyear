//! Managed the prediction history buffer, which stores past predicted and confirmed component states.
//!
//! The prediction history is used to:
//! 1. Compare predicted values with confirmed values from the server to detect mismatches
//! 2. Rollback to a past state and replay the simulation
//! 3. Preserve confirmed values during rollback so we can snap to them during re-simulation

use crate::rollback::DeterministicPredicted;
use crate::{Predicted, SyncComponent};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_ecs::component::Mutable;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_utils::prelude::DebugName;
use core::fmt::{self, Debug, Display};
use core::ops::Deref;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::{Rollback, SyncEvent};
use lightyear_replication::prelude::PreSpawned;
use lightyear_sync::prelude::InputTimelineConfig;
#[allow(unused_imports)]
use tracing::{debug, info, trace};

/// The state of a value in the prediction history.
///
/// We distinguish between:
/// - `Predicted` - a value that was computed locally during prediction. Can be cleared during rollback.
/// - `Confirmed` - a value that was received from the server. Should be preserved during rollback.
#[derive(Debug, PartialEq, Clone, Default, Reflect)]
pub enum PredictionState<R> {
    #[default]
    /// The component was removed (predicted locally)
    Removed,
    /// The component was removed and this was confirmed by the server
    ConfirmedRemoved,
    /// The value was predicted locally
    Predicted(R),
    /// The value was confirmed by the server
    Confirmed(R),
}

impl<R> PredictionState<R> {
    /// Returns true if this state is confirmed (received from the server)
    pub fn is_confirmed(&self) -> bool {
        matches!(
            self,
            PredictionState::Confirmed(_) | PredictionState::ConfirmedRemoved
        )
    }

    /// Returns true if this state is predicted (computed locally)
    pub fn is_predicted(&self) -> bool {
        matches!(
            self,
            PredictionState::Predicted(_) | PredictionState::Removed
        )
    }

    /// Returns true if the component exists (not removed)
    pub fn is_present(&self) -> bool {
        matches!(
            self,
            PredictionState::Predicted(_) | PredictionState::Confirmed(_)
        )
    }

    pub fn into_value(self) -> Option<R> {
        match self {
            PredictionState::Predicted(r) | PredictionState::Confirmed(r) => Some(r),
            PredictionState::Removed | PredictionState::ConfirmedRemoved => None,
        }
    }

    /// Get the inner value if present
    pub fn value(&self) -> Option<&R> {
        match self {
            PredictionState::Predicted(r) | PredictionState::Confirmed(r) => Some(r),
            PredictionState::Removed | PredictionState::ConfirmedRemoved => None,
        }
    }

    /// Get the inner value if present (mutable)
    pub fn value_mut(&mut self) -> Option<&mut R> {
        match self {
            PredictionState::Predicted(r) | PredictionState::Confirmed(r) => Some(r),
            PredictionState::Removed | PredictionState::ConfirmedRemoved => None,
        }
    }

    /// Convert a predicted state to confirmed, keeping the same value
    pub fn to_confirmed(self) -> Self {
        match self {
            PredictionState::Predicted(r) => PredictionState::Confirmed(r),
            PredictionState::Removed => PredictionState::ConfirmedRemoved,
            other => other,
        }
    }
}

impl<'w, R> From<&'w PredictionState<R>> for Option<&'w R> {
    fn from(val: &'w PredictionState<R>) -> Self {
        val.value()
    }
}

impl<R> From<PredictionState<R>> for Option<R> {
    fn from(val: PredictionState<R>) -> Self {
        match val {
            PredictionState::Removed | PredictionState::ConfirmedRemoved => None,
            PredictionState::Predicted(r) | PredictionState::Confirmed(r) => Some(r),
        }
    }
}

/// Holds the history of the component value at every tick, distinguishing between
/// predicted values (computed locally) and confirmed values (received from the server).
///
/// The key invariant is that **confirmed values are preserved during rollback**.
/// This allows us to:
/// 1. Rollback to a past tick
/// 2. During re-simulation, snap to confirmed values when we reach their tick
/// 3. Avoid re-predicting values we already know are correct from the server
#[derive(Component, Debug, Reflect)]
pub struct PredictionHistory<C> {
    /// Locally computed values. Front = oldest, back = most recent.
    /// These are cleared and rebuilt during rollback.
    predicted: VecDeque<(Tick, PredictionState<C>)>,
    /// Server-authoritative values. Front = oldest, back = most recent.
    /// These are preserved during rollback and can arrive out of order.
    confirmed: VecDeque<(Tick, PredictionState<C>)>,
}

impl<C> Default for PredictionHistory<C> {
    fn default() -> Self {
        Self {
            predicted: VecDeque::new(),
            confirmed: VecDeque::new(),
        }
    }
}

impl<C: Debug + Clone> Display for PredictionHistory<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PredictionHistory[")?;
        for (i, (tick, state)) in self.merged_entries().iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            let state_char = match state {
                PredictionState::Predicted(_) => "P",
                PredictionState::Confirmed(_) => "C",
                PredictionState::Removed => "R",
                PredictionState::ConfirmedRemoved => "CR",
            };
            write!(f, "{:?}:{}", tick, state_char)?;
        }
        write!(f, "]")
    }
}

impl<C> PredictionHistory<C> {
    pub fn len(&self) -> usize {
        self.predicted.len() + self.confirmed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.predicted.is_empty() && self.confirmed.is_empty()
    }

    /// Oldest value in the buffer
    pub fn oldest(&self) -> Option<&(Tick, PredictionState<C>)> {
        match (self.predicted.front(), self.confirmed.front()) {
            (Some(predicted), Some(confirmed)) => {
                if predicted.0 <= confirmed.0 {
                    Some(predicted)
                } else {
                    Some(confirmed)
                }
            }
            (Some(predicted), None) => Some(predicted),
            (None, Some(confirmed)) => Some(confirmed),
            (None, None) => None,
        }
    }

    /// Oldest locally predicted value in the buffer.
    pub fn oldest_predicted(&self) -> Option<&(Tick, PredictionState<C>)> {
        self.predicted.front()
    }

    /// Most recent value in the buffer
    pub fn most_recent(&self) -> Option<&(Tick, PredictionState<C>)> {
        match (self.predicted.back(), self.confirmed.back()) {
            (Some(predicted), Some(confirmed)) => {
                if confirmed.0 >= predicted.0 {
                    Some(confirmed)
                } else {
                    Some(predicted)
                }
            }
            (Some(predicted), None) => Some(predicted),
            (None, Some(confirmed)) => Some(confirmed),
            (None, None) => None,
        }
    }

    /// For unit tests
    #[doc(hidden)]
    pub fn buffer(&self) -> Vec<(Tick, PredictionState<C>)>
    where
        C: Clone,
    {
        self.merged_entries()
    }

    /// Get the value at the specified tick (returns the most recent value <= tick)
    pub fn get(&self, tick: Tick) -> Option<&C> {
        self.get_state(tick).and_then(PredictionState::value)
    }

    /// Get the full state at the specified tick
    pub fn get_state(&self, tick: Tick) -> Option<&PredictionState<C>> {
        match (
            Self::latest_at_or_before(&self.predicted, tick),
            Self::latest_at_or_before(&self.confirmed, tick),
        ) {
            (Some((predicted_tick, predicted)), Some((confirmed_tick, confirmed))) => {
                if confirmed_tick >= predicted_tick {
                    Some(confirmed)
                } else {
                    Some(predicted)
                }
            }
            (Some((_, predicted)), None) => Some(predicted),
            (None, Some((_, confirmed))) => Some(confirmed),
            (None, None) => None,
        }
    }

    /// Get the confirmed value exactly at the given tick, if one exists.
    pub fn get_confirmed_at(&self, tick: Tick) -> Option<&PredictionState<C>> {
        let partition = self.confirmed.partition_point(|(t, _)| *t < tick);
        self.confirmed
            .get(partition)
            .filter(|(t, _)| *t == tick)
            .map(|(_, state)| state)
    }

    /// Get the first confirmed value at or after the given tick.
    pub fn get_confirmed_at_or_after(&self, tick: Tick) -> Option<(Tick, &PredictionState<C>)> {
        let partition = self.confirmed.partition_point(|(t, _)| *t < tick);
        self.confirmed.get(partition).map(|(t, state)| (*t, state))
    }

    /// Get the last confirmed value in the history (most recent confirmed value).
    pub fn last_confirmed(&self) -> Option<&PredictionState<C>> {
        self.confirmed.back().map(|(_, state)| state)
    }

    /// Clear the entire history
    pub fn clear(&mut self) {
        self.predicted.clear();
        self.confirmed.clear();
    }

    /// Add a predicted value (computed locally)
    pub fn add_predicted(&mut self, tick: Tick, value: Option<C>) {
        self.add_predicted_state(
            tick,
            match value {
                Some(value) => PredictionState::Predicted(value),
                None => PredictionState::Removed,
            },
        );
    }

    /// Add a confirmed value at the given tick
    ///
    /// This is used in situations where we know the value is unchanged (e.g., a completed
    /// mutate tick confirms no mutation).
    /// Returns true if a new confirmed value was added, false otherwise.
    pub fn add_confirmed_unchanged(&mut self, tick: Tick) -> bool
    where
        C: Clone,
    {
        let Some((existing_tick, existing_state)) =
            Self::latest_at_or_before(&self.confirmed, tick)
        else {
            return false;
        };

        if existing_tick == tick {
            return false;
        }

        self.insert_confirmed_at_tick(tick, existing_state.clone());
        true
    }

    /// Add a confirmed value (received from the server)
    pub fn add_confirmed(&mut self, tick: Tick, value: Option<C>) {
        let state = match value {
            Some(value) => PredictionState::Confirmed(value),
            None => PredictionState::ConfirmedRemoved,
        };
        self.insert_confirmed_at_tick(tick, state);
    }

    /// Get the value immediately before `tick`.
    ///
    /// Used to efficiently get the previous value when doing correction. Confirmed values sort
    /// after predicted values at the same tick, so both states can coexist without overwriting.
    pub fn previous_value_before_tick(&self, tick: Tick) -> Option<&C> {
        let mut predicted_partition = self
            .predicted
            .partition_point(|(entry_tick, _)| *entry_tick <= tick);
        let mut confirmed_partition = self
            .confirmed
            .partition_point(|(entry_tick, _)| *entry_tick <= tick);

        let latest = self.take_latest_entry(&mut predicted_partition, &mut confirmed_partition);
        match latest {
            Some((entry_tick, state)) if entry_tick < tick => state.value(),
            Some(_) => self
                .take_latest_entry(&mut predicted_partition, &mut confirmed_partition)
                .and_then(|(_, state)| state.value()),
            None => None,
        }
    }

    #[doc(hidden)]
    pub fn second_most_recent(&self, tick: Tick) -> Option<&C> {
        self.previous_value_before_tick(tick)
    }

    /// Add a value with a specific state (for predicted values, appends to end)
    fn add_predicted_state(&mut self, tick: Tick, state: PredictionState<C>) {
        if let Some((last_tick, _)) = self.predicted.back()
            && *last_tick == tick
        {
            // Replace the existing value at this tick
            self.predicted.pop_back();
        }
        self.predicted.push_back((tick, state));
    }

    /// Insert a value at the correct position in the buffer (maintaining tick order).
    /// This is used for values that might arrive out of order.
    /// If a value already exists at this tick, it will be replaced.
    fn insert_confirmed_at_tick(&mut self, tick: Tick, state: PredictionState<C>) {
        // Find the position where this tick should be inserted
        let pos = self.confirmed.partition_point(|(t, _)| *t < tick);

        // Check if there's already a value at this exact tick
        if pos < self.confirmed.len() && self.confirmed[pos].0 == tick {
            // Replace the existing value
            self.confirmed[pos] = (tick, state);
        } else {
            // Insert at the correct position
            self.confirmed.insert(pos, (tick, state));
        }
    }

    /// Update ticks in case of a TickEvent (client tick changed)
    pub fn update_ticks(&mut self, delta: i32) {
        self.predicted.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
        self.confirmed.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
    }

    /// Pop the oldest value in the history
    pub fn pop(&mut self) -> Option<(Tick, PredictionState<C>)> {
        match (self.predicted.front(), self.confirmed.front()) {
            (Some(predicted), Some(confirmed)) => {
                if predicted.0 <= confirmed.0 {
                    self.predicted.pop_front()
                } else {
                    self.confirmed.pop_front()
                }
            }
            (Some(_), None) => self.predicted.pop_front(),
            (None, Some(_)) => self.confirmed.pop_front(),
            (None, None) => None,
        }
    }

    /// Clear all values strictly older than the specified tick
    pub fn clear_until_tick(&mut self, tick: Tick) {
        Self::clear_queue_until_tick(&mut self.predicted, tick);
        Self::clear_queue_until_tick(&mut self.confirmed, tick);
    }

    fn latest_at_or_before(
        buffer: &VecDeque<(Tick, PredictionState<C>)>,
        tick: Tick,
    ) -> Option<(Tick, &PredictionState<C>)> {
        let partition = buffer.partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        if partition == 0 {
            return None;
        }
        buffer
            .get(partition - 1)
            .map(|(tick, state)| (*tick, state))
    }

    fn clear_queue_until_tick(buffer: &mut VecDeque<(Tick, PredictionState<C>)>, tick: Tick) {
        let partition = buffer.partition_point(|(buffer_tick, _)| buffer_tick < &tick);
        if partition > 0 {
            buffer.drain(0..partition);
        }
    }

    fn clear_queue_through_tick(buffer: &mut VecDeque<(Tick, PredictionState<C>)>, tick: Tick) {
        let partition = buffer.partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        if partition > 0 {
            buffer.drain(0..partition);
        }
    }

    fn take_latest_entry(
        &self,
        predicted_partition: &mut usize,
        confirmed_partition: &mut usize,
    ) -> Option<(Tick, &PredictionState<C>)> {
        match (
            predicted_partition
                .checked_sub(1)
                .and_then(|index| self.predicted.get(index)),
            confirmed_partition
                .checked_sub(1)
                .and_then(|index| self.confirmed.get(index)),
        ) {
            (Some((predicted_tick, predicted)), Some((confirmed_tick, confirmed))) => {
                if confirmed_tick >= predicted_tick {
                    *confirmed_partition -= 1;
                    Some((*confirmed_tick, confirmed))
                } else {
                    *predicted_partition -= 1;
                    Some((*predicted_tick, predicted))
                }
            }
            (Some((predicted_tick, predicted)), None) => {
                *predicted_partition -= 1;
                Some((*predicted_tick, predicted))
            }
            (None, Some((confirmed_tick, confirmed))) => {
                *confirmed_partition -= 1;
                Some((*confirmed_tick, confirmed))
            }
            (None, None) => None,
        }
    }
}

impl<C: Clone> PredictionHistory<C> {
    fn merged_entries(&self) -> Vec<(Tick, PredictionState<C>)> {
        let mut entries = Vec::with_capacity(self.len());
        entries.extend(self.predicted.iter().cloned());
        entries.extend(self.confirmed.iter().cloned());
        entries.sort_by_key(|(tick, state)| (*tick, state.is_confirmed()));
        entries
    }

    /// Clear the history of values strictly older than the specified tick,
    /// and return the value at the specified tick.
    ///
    /// This is similar to HistoryBuffer::pop_until_tick but for PredictionHistory.
    pub fn pop_until_tick(&mut self, tick: Tick) -> Option<PredictionState<C>> {
        let res = self.get_state(tick).cloned();
        Self::clear_queue_through_tick(&mut self.predicted, tick);
        Self::clear_queue_through_tick(&mut self.confirmed, tick);
        if let Some(ref state) = res {
            if state.is_confirmed() {
                self.confirmed.push_front((tick, state.clone()));
            } else {
                self.predicted.push_front((tick, state.clone()));
            }
        }
        res
    }

    /// Clear all predicted values that are more recent than the rollback tick,
    /// while preserving confirmed values.
    pub fn clear_predicted_from(&mut self, rollback_tick: Tick) {
        let partition = self
            .predicted
            .partition_point(|(tick, _)| *tick <= rollback_tick);
        self.predicted.truncate(partition);
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
        trace!(
            "Prediction history updated for {:?} with tick delta {:?}",
            DebugName::type_name::<C>(),
            trigger.tick_delta
        );
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
/// `C` has just been applied via an init message, seed the history with a
/// confirmed entry at the server tick that produced the init.
///
/// # Why seeding is needed
///
/// Replicon reads entity markers on the empty newly-spawned entity BEFORE
/// init components are applied. As a result, the marker-gated `write_history`
/// function does NOT fire for init messages — the component value is written
/// directly to the entity via the default write, and `PredictionHistory<C>`
/// gets no confirmed entry for the init tick. We plug that hole here.
///
/// # Once-only semantics
///
/// Seeding only happens when we are creating the history on this observation
/// — if `PredictionHistory<C>` is already present, the markers were added
/// in a different order (e.g. after the component was first added and then
/// mutated by local prediction) and we must not overwrite those predicted
/// values with a stale confirmed snapshot.
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
        "Add prediction history for {:?} on entity {:?}",
        DebugName::type_name::<C>(),
        trigger.entity
    );
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
        // Skip if history already exists — either another observer run
        // created it, or local prediction already populated it and we
        // must not overwrite predicted values.
        if entity_mut.contains::<PredictionHistory<C>>() {
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
        let mut history = PredictionHistory::<C>::default();
        if let Some((tick, component)) = seed {
            trace!(
                ?entity,
                ?tick,
                component = ?DebugName::type_name::<C>(),
                "seeding PredictionHistory with confirmed value from init message"
            );
            history.add_confirmed(tick, Some(component));
        }
        entity_mut.insert(history);
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
    mut query: Query<(Entity, Option<&mut C>, &PredictionHistory<C>), With<Predicted>>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|(entity, component, history)| {
        // Check if there's a confirmed value at exactly this tick
        if let Some(confirmed_state) = history.get_confirmed_at(tick) {
            match confirmed_state.value() {
                Some(confirmed_value) => {
                    // Snap to the confirmed value
                    if let Some(mut comp) = component {
                        if comp.deref() != confirmed_value {
                            trace!(
                                ?entity,
                                ?tick,
                                "Snapping to confirmed value during rollback for {:?}",
                                DebugName::type_name::<C>()
                            );
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
                None => {
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

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    #[test]
    fn test_predicted_confirmed_distinction() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_predicted(Tick(1), Some(TestValue(1.0)));
        history.add_confirmed(Tick(3), Some(TestValue(3.0)));
        history.add_predicted(Tick(5), Some(TestValue(5.0)));

        // get_confirmed_at should only return confirmed values
        assert!(history.get_confirmed_at(Tick(1)).is_none()); // predicted
        assert!(history.get_confirmed_at(Tick(3)).is_some()); // confirmed
        assert!(history.get_confirmed_at(Tick(5)).is_none()); // predicted
    }

    #[test]
    fn test_clear_predicted_from_preserves_confirmed() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_predicted(Tick(1), Some(TestValue(1.0)));
        history.add_confirmed(Tick(3), Some(TestValue(3.0)));
        history.add_predicted(Tick(5), Some(TestValue(5.0)));
        history.add_confirmed(Tick(7), Some(TestValue(7.0)));
        history.add_predicted(Tick(9), Some(TestValue(9.0)));

        // Clear predicted values from tick 4 onwards
        let restore_value = history.get(Tick(4)).cloned();
        history.clear_predicted_from(Tick(4));

        // Should restore to the value at tick 4 (which is the confirmed value at tick 3)
        assert!(matches!(restore_value, Some(TestValue(v)) if v == 3.0));

        // Confirmed value at tick 7 should be preserved
        assert!(history.get_confirmed_at(Tick(7)).is_some());

        // Predicted values at tick 5 and 9 should be removed
        // (we can check by seeing the buffer doesn't have them)
        let buffer = history.buffer();
        let has_tick_5 = buffer.iter().any(|(t, _)| *t == Tick(5));
        let has_tick_9 = buffer.iter().any(|(t, _)| *t == Tick(9));
        assert!(!has_tick_5);
        assert!(!has_tick_9);
    }

    #[test]
    fn test_add_confirmed_unchanged_preserves_existing_history() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_predicted(Tick(21), Some(TestValue(21.0)));
        history.add_confirmed(Tick(22), Some(TestValue(100.0)));
        history.add_predicted(Tick(23), Some(TestValue(23.0)));
        history.add_predicted(Tick(24), Some(TestValue(24.0)));

        assert!(history.add_confirmed_unchanged(Tick(25)));
        assert_eq!(
            history
                .get_confirmed_at(Tick(22))
                .and_then(|s| s.value())
                .unwrap()
                .0,
            100.0
        );
        assert_eq!(
            history
                .get_confirmed_at(Tick(25))
                .and_then(|s| s.value())
                .unwrap()
                .0,
            100.0
        );
        assert_eq!(history.get(Tick(22)).unwrap().0, 100.0);
        assert_eq!(history.get(Tick(24)).unwrap().0, 24.0);
    }

    #[test]
    fn test_add_confirmed_unchanged_at_same_tick_is_noop() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_confirmed(Tick(22), Some(TestValue(100.0)));
        let before_len = history.len();

        assert!(!history.add_confirmed_unchanged(Tick(22)));
        assert_eq!(history.len(), before_len);
        assert_eq!(
            history
                .get_confirmed_at(Tick(22))
                .and_then(|s| s.value())
                .unwrap()
                .0,
            100.0
        );
    }

    #[test]
    fn predicted_insert_after_future_confirmed_preserves_tick_order() {
        let mut history = PredictionHistory::<TestValue>::default();

        history.add_predicted(Tick(670), Some(TestValue(670.0)));
        history.add_confirmed(Tick(691), Some(TestValue(691.0)));
        history.add_predicted(Tick(676), Some(TestValue(676.0)));

        assert_eq!(history.buffer().len(), 3);
        assert_eq!(history.buffer()[0].0, Tick(670));
        assert_eq!(history.buffer()[1].0, Tick(676));
        assert_eq!(history.buffer()[2].0, Tick(691));
        assert_eq!(
            history.most_recent().map(|(tick, _)| *tick),
            Some(Tick(691))
        );
        assert_eq!(history.get(Tick(680)).unwrap().0, 676.0);
        assert_eq!(history.get(Tick(691)).unwrap().0, 691.0);
        assert_eq!(history.second_most_recent(Tick(676)).unwrap().0, 670.0);
        assert_eq!(history.second_most_recent(Tick(680)).unwrap().0, 676.0);
        assert_eq!(history.second_most_recent(Tick(691)).unwrap().0, 676.0);
    }
}
