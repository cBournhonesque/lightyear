//! Managed the prediction history buffer, which stores past predicted and confirmed component states.
//!
//! The prediction history is used to:
//! 1. Compare predicted values with confirmed values from the server to detect mismatches
//! 2. Rollback to a past state and replay the simulation
//! 3. Preserve confirmed values during rollback so we can snap to them during re-simulation

use crate::Predicted;
use crate::rollback::DeterministicPredicted;
use alloc::collections::VecDeque;
use bevy_ecs::prelude::*;
use bevy_ecs::component::Mutable;
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
        matches!(self, PredictionState::Confirmed(_) | PredictionState::ConfirmedRemoved)
    }

    /// Returns true if this state is predicted (computed locally)
    pub fn is_predicted(&self) -> bool {
        matches!(self, PredictionState::Predicted(_) | PredictionState::Removed)
    }

    /// Returns true if the component exists (not removed)
    pub fn is_present(&self) -> bool {
        matches!(self, PredictionState::Predicted(_) | PredictionState::Confirmed(_))
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
    /// Queue containing the history. Front = oldest, back = most recent.
    /// Only stores ticks where the value changed.
    buffer: VecDeque<(Tick, PredictionState<C>)>,
}

impl<C> Default for PredictionHistory<C> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
        }
    }
}

impl<C: Debug> Display for PredictionHistory<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PredictionHistory[")?;
        for (i, (tick, state)) in self.buffer.iter().enumerate() {
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
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Oldest value in the buffer
    pub fn oldest(&self) -> Option<&(Tick, PredictionState<C>)> {
        self.buffer.front()
    }

    /// Most recent value in the buffer
    pub fn most_recent(&self) -> Option<&(Tick, PredictionState<C>)> {
        self.buffer.back()
    }

    /// Get the value at the specified tick (returns the most recent value <= tick)
    pub fn get(&self, tick: Tick) -> Option<&C> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        if partition == 0 {
            return None;
        }
        self.buffer.get(partition - 1).and_then(|(_, state)| state.value())
    }

    /// Get the full state at the specified tick
    pub fn get_state(&self, tick: Tick) -> Option<&PredictionState<C>> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        if partition == 0 {
            return None;
        }
        self.buffer.get(partition - 1).map(|(_, state)| state)
    }

    /// Get the confirmed value exactly at the given tick, if one exists.
    pub fn get_confirmed_at(&self, tick: Tick) -> Option<&PredictionState<C>> {
        self.buffer
            .iter()
            .find(|(t, state)| *t == tick && state.is_confirmed())
            .map(|(_, state)| state)
    }

    /// Get the first confirmed value at or after the given tick.
    pub fn get_confirmed_at_or_after(&self, tick: Tick) -> Option<(Tick, &PredictionState<C>)> {
        self.buffer
            .iter()
            .find(|(t, state)| *t >= tick && state.is_confirmed())
            .map(|(t, state)| (*t, state))
    }

    /// Get the last confirmed value in the history (most recent confirmed value).
    pub fn last_confirmed(&self) -> Option<&PredictionState<C>> {
        self.buffer
            .iter()
            .rev()
            .find(|(_, state)| state.is_confirmed())
            .map(|(_, state)| state)
    }

    /// Clear the entire history
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Add a predicted value (computed locally)
    pub fn add_predicted(&mut self, tick: Tick, value: Option<C>) {
        self.add_state(
            tick,
            match value {
                Some(value) => PredictionState::Predicted(value),
                None => PredictionState::Removed,
            },
        );
    }

    /// Add a confirmed value at the given tick
    ///
    /// This is used in situations where we know the value is unchanged (e.g., ServerMutateTicks confirms no mutation).
    /// Returns true if a new confirmed value was added, false otherwise.
    pub fn add_confirmed_unchanged(&mut self, tick: Tick) -> bool {
        // Pop all values before the given tick, keeping track of the most recent confirmed value before tick
        while let Some((t, state)) = self.buffer.pop_front() {
            if t < tick {
                // If it's confirmed, add it as the confirmed value at 'tick'
                if state.is_confirmed() {
                    self.clear_until_tick(tick);
                    self.insert_at_tick(tick, state);
                    return true;
                }
            } else {
                break;
            }
        }
        false
    }

    /// Add a confirmed value (received from the server)
    pub fn add_confirmed(&mut self, tick: Tick, value: Option<C>) {
        let state = match value {
            Some(value) => PredictionState::Confirmed(value),
            None => PredictionState::ConfirmedRemoved,
        };
        self.insert_at_tick(tick, state);
    }

    #[doc(hidden)]
    /// Get the second most recent value in the buffer, knowing that tick is the current tick.
    ///
    /// Used to efficiently get the previous value when doing correction.
    pub fn second_most_recent(&self, tick: Tick) -> Option<&C> {
        let (most_recent_tick, most_recent) = self.most_recent()?;
        if *most_recent_tick < tick {
            return most_recent.into();
        }
        let len = self.buffer.len();
        if len < 2 {
            return None;
        }
        self.buffer.get(len - 2).and_then(|(_, x)| x.into())
    }

    /// Add a value with a specific state (for predicted values, appends to end)
    fn add_state(&mut self, tick: Tick, state: PredictionState<C>) {
        if let Some((last_tick, _)) = self.buffer.back() {
            if *last_tick == tick {
                // Replace the existing value at this tick
                self.buffer.pop_back();
            }
        }
        self.buffer.push_back((tick, state));
    }

    /// Insert a value at the correct position in the buffer (maintaining tick order).
    /// This is used for confirmed values which might arrive out of order.
    /// If a value already exists at this tick, it will be replaced.
    fn insert_at_tick(&mut self, tick: Tick, state: PredictionState<C>) {
        // Find the position where this tick should be inserted
        let pos = self.buffer.partition_point(|(t, _)| *t < tick);

        // Check if there's already a value at this exact tick
        if pos < self.buffer.len() && self.buffer[pos].0 == tick {
            // Replace the existing value
            self.buffer[pos] = (tick, state);
        } else {
            // Insert at the correct position
            self.buffer.insert(pos, (tick, state));
        }
    }

    /// Update ticks in case of a TickEvent (client tick changed)
    pub fn update_ticks(&mut self, delta: i16) {
        self.buffer.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
    }

    /// Pop the oldest value in the history
    pub fn pop(&mut self) -> Option<(Tick, PredictionState<C>)> {
        self.buffer.pop_front()
    }

    /// Clear all values strictly older than the specified tick
    pub fn clear_until_tick(&mut self, tick: Tick) {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        if partition > 0 {
            self.buffer.drain(0..partition);
        }
    }
}

impl<C: Clone> PredictionHistory<C> {
    /// Clear the history of values strictly older than the specified tick,
    /// and return the value at the specified tick.
    ///
    /// This is similar to HistoryBuffer::pop_until_tick but for PredictionHistory.
    pub fn pop_until_tick(&mut self, tick: Tick) -> Option<PredictionState<C>> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);

        if partition == 0 {
            return None;
        }

        // Drain all elements strictly older than the tick
        self.buffer.drain(0..(partition - 1));
        let res = self.buffer.pop_front().map(|(_, state)| state);

        // Re-add the value at tick to the buffer
        if let Some(ref state) = res {
            self.buffer.push_front((tick, state.clone()));
        }

        res
    }

    /// Clear all predicted values that are more recent than the rollback tick,
    /// while preserving confirmed values.
    pub fn clear_predicted_from(&mut self, rollback_tick: Tick) {
        self.buffer.retain(|(tick, state)| *tick <= rollback_tick || state.is_confirmed());
    }

}

// ============================================================================
// Systems
// ============================================================================

/// We store every update on the predicted entity in the PredictionHistory
///
/// This system only handles changes, removals are handled in `apply_component_removal`
pub(crate) fn update_prediction_history<T: Component + Clone>(
    mut query: Query<(Ref<T>, &mut PredictionHistory<T>)>,
    timeline: Res<LocalTimeline>,
) {
    // tick for which we will record the history (either the current client tick or the current rollback tick)
    let tick = timeline.tick();

    // update history if the predicted component changed
    for (component, mut history) in query.iter_mut() {
        // change detection works even when running the schedule for rollback
        if component.is_changed() {
            history.add_predicted(tick, Some(component.deref().clone()));
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
    }
}

/// If a predicted component gets added to [`Predicted`] entity, add a [`PredictionHistory`] component.
pub(crate) fn add_prediction_history<C: Component>(
    trigger: On<
        Add,
        (
            C,
            Predicted,
            PreSpawned,
            DeterministicPredicted,
        ),
    >,
    mut commands: Commands,
    query: Query<
        (),
        (
            Without<PredictionHistory<C>>,
            With<C>,
            Or<(
                With<Predicted>,
                With<PreSpawned>,
                With<DeterministicPredicted>,
            )>,
        ),
    >,
) {
    if query.get(trigger.entity).is_ok() {
        trace!(
            "Add prediction history for {:?} on entity {:?}",
            DebugName::type_name::<C>(),
            trigger.entity
        );
        commands
            .entity(trigger.entity)
            .insert(PredictionHistory::<C>::default());
    }
}

/// During rollback re-simulation, check if we have a confirmed value for this tick.
/// If so, snap the component to the confirmed value instead of using the predicted value.
pub(crate) fn snap_to_confirmed_during_rollback<C: Component<Mutability = Mutable> + Clone + PartialEq>(
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
        let restore_value = history.clear_predicted_from(Tick(4));

        // Should restore to the value at tick 4 (which is the confirmed value at tick 3)
        assert!(matches!(restore_value, Some(PredictionState::Confirmed(TestValue(v))) if v == 3.0));

        // Confirmed value at tick 7 should be preserved
        assert!(history.get_confirmed_at(Tick(7)).is_some());

        // Predicted values at tick 5 and 9 should be removed
        // (we can check by seeing the buffer doesn't have them)
        let has_tick_5 = history.buffer.iter().any(|(t, _)| *t == Tick(5));
        let has_tick_9 = history.buffer.iter().any(|(t, _)| *t == Tick(9));
        assert!(!has_tick_5);
        assert!(!has_tick_9);
    }
}
