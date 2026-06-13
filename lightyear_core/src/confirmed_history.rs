use crate::tick::Tick;
use alloc::collections::{VecDeque, vec_deque};
use alloc::vec::Vec;
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;

/// Authoritative state received from the remote for a component.
#[derive(Debug, PartialEq, Clone, Default, Reflect)]
pub enum ConfirmedState<C> {
    #[default]
    /// The authoritative component state is absent.
    Removed,
    /// The authoritative component state is present.
    Confirmed(C),
}

impl<C> ConfirmedState<C> {
    /// Returns true if the component exists in this state.
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }

    /// Get the inner value if present.
    pub fn value(&self) -> Option<&C> {
        match self {
            Self::Confirmed(value) => Some(value),
            Self::Removed => None,
        }
    }

    /// Get the inner value if present.
    pub fn into_value(self) -> Option<C> {
        match self {
            Self::Confirmed(value) => Some(value),
            Self::Removed => None,
        }
    }
}

impl<'w, C> From<&'w ConfirmedState<C>> for Option<&'w C> {
    fn from(value: &'w ConfirmedState<C>) -> Self {
        value.value()
    }
}

impl<C> From<ConfirmedState<C>> for Option<C> {
    fn from(value: ConfirmedState<C>) -> Self {
        value.into_value()
    }
}

/// A raw entry stored in [`ConfirmedHistory`].
#[derive(Debug, PartialEq, Clone, Reflect)]
pub(crate) enum ConfirmedHistoryState<C> {
    /// An explicit authoritative component state received from replication.
    Explicit(ConfirmedState<C>),
    /// The authoritative state is unchanged from the closest preceding entry.
    ///
    /// This is resolved dynamically. If a late explicit update is inserted before
    /// this tick, this entry will automatically resolve to that newer preceding
    /// state.
    SameAsPrecedent,
}

impl<C> ConfirmedHistoryState<C> {
    /// Return the explicit state stored in this raw entry.
    fn explicit_state(&self) -> Option<&ConfirmedState<C>> {
        match self {
            Self::Explicit(state) => Some(state),
            Self::SameAsPrecedent => None,
        }
    }

    fn is_same_as_precedent(&self) -> bool {
        matches!(self, Self::SameAsPrecedent)
    }
}

/// Stores authoritative component states received from the remote.
///
/// The buffer is ordered from oldest to newest. Entries are either explicit
/// authoritative updates/removals received from replication, or raw unchanged
/// markers when prediction/interpolation has external proof that the
/// authoritative state did not change at a tick.
#[derive(Component, Debug, Reflect)]
pub struct ConfirmedHistory<C> {
    buffer: VecDeque<(Tick, ConfirmedHistoryState<C>)>,
}

impl<C> Default for ConfirmedHistory<C> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
        }
    }
}

// This matches the historical interpolation behavior: tests compare anchor
// ticks and whether an anchor is unchanged, not component values.
impl<C> PartialEq for ConfirmedHistory<C> {
    fn eq(&self, other: &Self) -> bool {
        let self_entries: Vec<_> = self
            .buffer
            .iter()
            .map(|(tick, state)| (*tick, state.is_same_as_precedent()))
            .collect();
        let other_entries: Vec<_> = other
            .buffer
            .iter()
            .map(|(tick, state)| (*tick, state.is_same_as_precedent()))
            .collect();
        self_entries == other_entries
    }
}

impl<C> ConfirmedHistory<C> {
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn buffer_raw(&self) -> &VecDeque<(Tick, ConfirmedHistoryState<C>)> {
        &self.buffer
    }

    /// Get the n-th oldest tick in the buffer.
    pub fn get_nth_tick(&self, n: usize) -> Option<Tick> {
        self.buffer.get(n).map(|(tick, _)| *tick)
    }

    /// Get the n-th oldest resolved state in the buffer.
    pub fn get_nth_state(&self, n: usize) -> Option<(Tick, &ConfirmedState<C>)> {
        self.buffer
            .get(n)
            .and_then(|(tick, _)| self.resolve_state_at_index(n).map(|state| (*tick, state)))
    }

    /// The oldest present value in the history.
    pub fn start_present(&self) -> Option<(Tick, &C)> {
        self.get_nth_present(0)
    }

    /// The most recent present value in the history.
    pub fn newest_present(&self) -> Option<(Tick, &C)> {
        let index = self.buffer.len().checked_sub(1)?;
        let (tick, _) = self.buffer.get(index)?;
        self.resolve_state_at_index(index)
            .and_then(ConfirmedState::value)
            .map(|value| (*tick, value))
    }

    /// Get the n-th oldest entry if it is a present value.
    pub fn get_nth_present(&self, n: usize) -> Option<(Tick, &C)> {
        self.get_nth_state(n)
            .and_then(|(tick, state)| state.value().map(|value| (tick, value)))
    }

    /// Get the latest present value at or before `tick`.
    pub fn get_present(&self, tick: Tick) -> Option<&C> {
        self.get_state_at_or_before(tick)
            .and_then(ConfirmedState::value)
    }

    /// Get the latest authoritative state at or before `tick`.
    pub fn get_state_at_or_before(&self, tick: Tick) -> Option<&ConfirmedState<C>> {
        let index = self.index_at_or_before(tick)?;
        self.resolve_state_at_index(index)
    }

    /// Get the authoritative state exactly at `tick`.
    pub fn get_state_at(&self, tick: Tick) -> Option<&ConfirmedState<C>> {
        let pos = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick < tick);
        self.buffer
            .get(pos)
            .and_then(|(buffer_tick, _)| (*buffer_tick == tick).then_some(pos))
            .and_then(|index| self.resolve_state_at_index(index))
    }

    fn index_at_or_before(&self, tick: Tick) -> Option<usize> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        partition.checked_sub(1)
    }

    fn index_before(&self, tick: Tick) -> Option<usize> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick < tick);
        partition.checked_sub(1)
    }

    fn resolve_state_at_index(&self, index: usize) -> Option<&ConfirmedState<C>> {
        (0..=index).rev().find_map(|i| {
            self.buffer
                .get(i)
                .and_then(|(_, state)| state.explicit_state())
        })
    }

    fn state_before(&self, tick: Tick) -> Option<&ConfirmedState<C>> {
        let index = self.index_before(tick)?;
        self.resolve_state_at_index(index)
    }

    fn insert_raw(&mut self, tick: Tick, state: ConfirmedHistoryState<C>) {
        let pos = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick < tick);
        if pos < self.buffer.len() && self.buffer[pos].0 == tick {
            self.buffer[pos] = (tick, state);
        } else {
            self.buffer.insert(pos, (tick, state));
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Shift all stored ticks by `delta`.
    pub fn update_ticks(&mut self, delta: i32) {
        self.buffer.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
    }
}

impl<C: PartialEq> ConfirmedHistory<C> {
    /// Insert an authoritative state while preserving tick order.
    ///
    /// If the state is equal to the effective authoritative state immediately
    /// before `tick`, the raw entry is stored as
    /// an internal unchanged marker.
    pub fn insert(&mut self, tick: Tick, state: ConfirmedState<C>) {
        let entry = if self.state_before(tick).is_some_and(|prev| prev == &state) {
            ConfirmedHistoryState::SameAsPrecedent
        } else {
            ConfirmedHistoryState::Explicit(state)
        };
        self.insert_raw(tick, entry);
    }

    /// Insert a present authoritative value while preserving tick order.
    pub fn insert_present(&mut self, tick: Tick, value: C) {
        self.insert(tick, ConfirmedState::Confirmed(value));
    }

    /// Insert an authoritative removal while preserving tick order.
    pub fn insert_removed(&mut self, tick: Tick) {
        self.insert(tick, ConfirmedState::Removed);
    }

    /// Insert an authoritative state assuming `tick` is not older than the
    /// current newest sample.
    ///
    /// This avoids the binary search and middle insertion used by [`insert`],
    /// but is only correct for callers that already know updates are arriving
    /// in sorted order.
    ///
    /// # Safety
    ///
    /// The caller must ensure `tick` is not older than the current newest tick.
    /// Passing an older tick leaves the buffer out of order and breaks all
    /// lookup methods that rely on sorted ticks.
    ///
    /// [`insert`]: ConfirmedHistory::insert
    pub unsafe fn insert_assume_sorted(&mut self, tick: Tick, state: ConfirmedState<C>) {
        debug_assert!(
            self.buffer
                .back()
                .is_none_or(|(newest_tick, _)| tick >= *newest_tick),
            "insert_assume_sorted called with out-of-order tick"
        );
        if let Some((last_tick, _)) = self.buffer.back()
            && *last_tick == tick
        {
            self.buffer.pop_back();
        }
        let entry = if self.state_before(tick).is_some_and(|prev| prev == &state) {
            ConfirmedHistoryState::SameAsPrecedent
        } else {
            ConfirmedHistoryState::Explicit(state)
        };
        self.buffer.push_back((tick, entry));
    }

    /// Insert a present authoritative value assuming `tick` is not older than
    /// the current newest sample.
    ///
    /// # Safety
    ///
    /// The caller must ensure `tick` is not older than the current newest tick.
    pub unsafe fn insert_present_assume_sorted(&mut self, tick: Tick, value: C) {
        // SAFETY: This method's caller must uphold the sorted insertion precondition.
        unsafe { self.insert_assume_sorted(tick, ConfirmedState::Confirmed(value)) };
    }
}

impl<C> ConfirmedHistory<C> {
    /// Add an unchanged sample at `tick`.
    ///
    /// Use this when another system has proven that the authoritative component
    /// state did not change at `tick` even though no explicit component update
    /// was received.
    pub fn add_unchanged(&mut self, tick: Tick) -> bool {
        if self.get_state_at(tick).is_some() || self.state_before(tick).is_none() {
            return false;
        }
        self.insert_raw(tick, ConfirmedHistoryState::SameAsPrecedent);
        true
    }

    /// Advance the newest present value to `tick` for interpolation.
    ///
    /// Use this only when `tick` comes from a monotonic completeness signal,
    /// such as the latest completed mutate tick. Consecutive unchanged ticks
    /// update the same unchanged newest anchor instead of appending another
    /// marker.
    pub fn push_unchanged(&mut self, tick: Tick) -> Option<Tick> {
        let newest_index = self.buffer.len().checked_sub(1)?;
        let (newest_tick, newest_state) = self.buffer.get(newest_index)?;
        let newest_tick = *newest_tick;
        if tick <= newest_tick
            || self
                .resolve_state_at_index(newest_index)
                .and_then(ConfirmedState::value)
                .is_none()
        {
            return None;
        }

        if newest_state.is_same_as_precedent() {
            self.buffer.back_mut().unwrap().0 = tick;
        } else if !self.add_unchanged(tick) {
            return None;
        }
        Some(newest_tick)
    }
}

impl<C: Clone> ConfirmedHistory<C> {
    fn materialize_front_if_same_as_precedent(&mut self, state: ConfirmedState<C>) {
        if let Some((_, raw_state)) = self.buffer.front_mut()
            && raw_state.is_same_as_precedent()
        {
            *raw_state = ConfirmedHistoryState::Explicit(state);
        }
    }

    /// Pop the oldest present value in the history.
    pub fn pop_present(&mut self) -> Option<(Tick, C)> {
        let popped_state = self
            .buffer
            .front()
            .and_then(|(_, state)| state.explicit_state())
            .cloned();
        let popped = match self.buffer.pop_front() {
            Some((tick, ConfirmedHistoryState::Explicit(ConfirmedState::Confirmed(value)))) => {
                Some((tick, value))
            }
            Some((_, ConfirmedHistoryState::Explicit(ConfirmedState::Removed)))
            | Some((_, ConfirmedHistoryState::SameAsPrecedent))
            | None => None,
        };
        if let Some(state) = popped_state
            && self
                .buffer
                .front()
                .is_some_and(|(_, state)| state.is_same_as_precedent())
        {
            self.materialize_front_if_same_as_precedent(state);
        }
        popped
    }

    /// Clear all states strictly older than `tick`.
    pub fn clear_until_tick(&mut self, tick: Tick) {
        let state_at_cut = self.get_state_at_or_before(tick).cloned();
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick < &tick);
        if partition > 0 {
            self.buffer.drain(0..partition);
        }
        if let Some(state) = state_at_cut
            && self
                .buffer
                .front()
                .is_some_and(|(_, state)| state.is_same_as_precedent())
        {
            self.materialize_front_if_same_as_precedent(state);
        }
    }
}

/// The iterator contains the present values from oldest to most recent.
impl<'a, C> IntoIterator for &'a ConfirmedHistory<C> {
    type Item = (Tick, &'a C);
    type IntoIter = ConfirmedHistoryIter<'a, C>;

    fn into_iter(self) -> Self::IntoIter {
        ConfirmedHistoryIter {
            iter: self.buffer.iter(),
            current_state: None,
        }
    }
}

pub struct ConfirmedHistoryIter<'a, C> {
    iter: vec_deque::Iter<'a, (Tick, ConfirmedHistoryState<C>)>,
    current_state: Option<&'a ConfirmedState<C>>,
}

impl<'a, C> Iterator for ConfirmedHistoryIter<'a, C> {
    type Item = (Tick, &'a C);

    fn next(&mut self) -> Option<Self::Item> {
        for (tick, state) in self.iter.by_ref() {
            if let Some(explicit_state) = state.explicit_state() {
                self.current_state = Some(explicit_state);
            }
            if let Some(ConfirmedState::Confirmed(value)) = self.current_state {
                return Some((*tick, value));
            }
        }
        None
    }
}

/// The iterator contains the present values from oldest to most recent.
impl<C: Clone> IntoIterator for ConfirmedHistory<C> {
    type Item = (Tick, C);
    type IntoIter = ConfirmedHistoryIntoIter<C>;

    fn into_iter(self) -> Self::IntoIter {
        ConfirmedHistoryIntoIter {
            iter: self.buffer.into_iter(),
            current_state: None,
        }
    }
}

pub struct ConfirmedHistoryIntoIter<C> {
    iter: vec_deque::IntoIter<(Tick, ConfirmedHistoryState<C>)>,
    current_state: Option<ConfirmedState<C>>,
}

impl<C: Clone> Iterator for ConfirmedHistoryIntoIter<C> {
    type Item = (Tick, C);

    fn next(&mut self) -> Option<Self::Item> {
        for (tick, state) in self.iter.by_ref() {
            if let ConfirmedHistoryState::Explicit(explicit_state) = state {
                self.current_state = Some(explicit_state);
            }
            if let Some(ConfirmedState::Confirmed(value)) = self.current_state.as_ref() {
                return Some((tick, value.clone()));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use test_log::test;

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    fn explicit(value: f32) -> ConfirmedHistoryState<TestValue> {
        ConfirmedHistoryState::Explicit(ConfirmedState::Confirmed(TestValue(value)))
    }

    fn removed() -> ConfirmedHistoryState<TestValue> {
        ConfirmedHistoryState::Explicit(ConfirmedState::Removed)
    }

    fn same() -> ConfirmedHistoryState<TestValue> {
        ConfirmedHistoryState::SameAsPrecedent
    }

    fn effective_value_at(history: &ConfirmedHistory<TestValue>, tick: Tick) -> Option<f32> {
        history.get_present(tick).map(|value| value.0)
    }

    #[test]
    fn insert_supports_out_of_order_exact_samples() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(5), TestValue(5.0));
        history.insert_present(Tick(1), TestValue(1.0));
        history.insert_removed(Tick(3));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(3), removed()),
                (Tick(5), explicit(5.0)),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(2)), Some(1.0));
        assert!(history.get_present(Tick(3)).is_none());
        assert_eq!(effective_value_at(&history, Tick(5)), Some(5.0));
    }

    #[test]
    fn add_unchanged_preserves_future_samples() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(2), TestValue(2.0));
        history.insert_present(Tick(10), TestValue(10.0));

        assert!(history.add_unchanged(Tick(5)));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(2), explicit(2.0)),
                (Tick(5), same()),
                (Tick(10), explicit(10.0)),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(5)), Some(2.0));
        assert_eq!(effective_value_at(&history, Tick(9)), Some(2.0));
        assert_eq!(effective_value_at(&history, Tick(10)), Some(10.0));
    }

    #[test]
    fn unchanged_in_middle_tracks_late_preceding_insert() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        history.insert_present(Tick(3), TestValue(2.0));
        assert!(history.add_unchanged(Tick(7)));

        history.insert_present(Tick(5), TestValue(3.0));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(3), explicit(2.0)),
                (Tick(5), explicit(3.0)),
                (Tick(7), same()),
            ])
        );
        assert_eq!(
            effective_value_at(&history, Tick(7)),
            Some(3.0),
            "the unchanged tick should resolve to the late C@5 update"
        );
    }

    #[test]
    fn explicit_same_value_after_unchanged_is_stored_as_unchanged() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        history.insert_present(Tick(3), TestValue(2.0));
        assert!(history.add_unchanged(Tick(7)));

        history.insert_present(Tick(9), TestValue(2.0));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(3), explicit(2.0)),
                (Tick(7), same()),
                (Tick(9), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(9)), Some(2.0));
    }

    #[test]
    fn explicit_different_value_after_unchanged_is_stored_explicitly() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        history.insert_present(Tick(3), TestValue(2.0));
        assert!(history.add_unchanged(Tick(7)));

        history.insert_present(Tick(9), TestValue(3.0));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(3), explicit(2.0)),
                (Tick(7), same()),
                (Tick(9), explicit(3.0)),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(7)), Some(2.0));
        assert_eq!(effective_value_at(&history, Tick(9)), Some(3.0));
    }

    #[test]
    fn out_of_order_same_value_is_stored_as_unchanged() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        history.insert_present(Tick(3), TestValue(2.0));
        assert!(history.add_unchanged(Tick(7)));

        history.insert_present(Tick(2), TestValue(1.0));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(2), same()),
                (Tick(3), explicit(2.0)),
                (Tick(7), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(2)), Some(1.0));
        assert_eq!(effective_value_at(&history, Tick(7)), Some(2.0));
    }

    #[test]
    fn push_unchanged_slides_newest_anchor() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(2), TestValue(2.0));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));
        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![(Tick(2), explicit(2.0)), (Tick(8), same()),])
        );
        assert_eq!(effective_value_at(&history, Tick(8)), Some(2.0));
    }

    #[test]
    fn out_of_order_insert_before_unchanged_anchor_preserves_effective_value() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(2), TestValue(2.0));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));

        history.insert_present(Tick(1), TestValue(1.0));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(2), explicit(2.0)),
                (Tick(5), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(5)), Some(2.0));

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(1), explicit(1.0)),
                (Tick(2), explicit(2.0)),
                (Tick(8), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(8)), Some(2.0));
    }

    #[test]
    fn out_of_order_insert_before_unchanged_tick_updates_effective_value() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(2), TestValue(2.0));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));

        history.insert_present(Tick(3), TestValue(3.0));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(2), explicit(2.0)),
                (Tick(3), explicit(3.0)),
                (Tick(5), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(5)), Some(3.0));

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(2), explicit(2.0)),
                (Tick(3), explicit(3.0)),
                (Tick(8), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(8)), Some(3.0));
    }

    #[test]
    fn explicit_insert_at_unchanged_tick_replaces_raw_marker() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(2), TestValue(2.0));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));

        history.insert_present(Tick(5), TestValue(5.0));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![(Tick(2), explicit(2.0)), (Tick(5), explicit(5.0)),])
        );

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![
                (Tick(2), explicit(2.0)),
                (Tick(5), explicit(5.0)),
                (Tick(8), same()),
            ])
        );
        assert_eq!(effective_value_at(&history, Tick(8)), Some(5.0));
    }

    #[test]
    fn pop_materializes_unchanged_front() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        assert!(history.add_unchanged(Tick(3)));
        assert!(history.add_unchanged(Tick(7)));

        assert_eq!(history.pop_present(), Some((Tick(1), TestValue(1.0))));
        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![(Tick(3), explicit(1.0)), (Tick(7), same())])
        );
        assert_eq!(effective_value_at(&history, Tick(7)), Some(1.0));
    }

    #[test]
    fn clear_until_tick_materializes_unchanged_front() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert_present(Tick(1), TestValue(1.0));
        assert!(history.add_unchanged(Tick(3)));
        assert!(history.add_unchanged(Tick(7)));

        history.clear_until_tick(Tick(3));

        assert_eq!(
            history.buffer_raw(),
            &VecDeque::from(vec![(Tick(3), explicit(1.0)), (Tick(7), same())])
        );
        assert_eq!(effective_value_at(&history, Tick(7)), Some(1.0));
    }
}
