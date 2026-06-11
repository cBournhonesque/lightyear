use crate::tick::Tick;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;
use core::iter::FilterMap;

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

/// Stores authoritative component states received from the remote.
///
/// The buffer is ordered from oldest to newest. Most entries are explicit
/// authoritative updates or removals received from replication. Some prediction
/// and interpolation systems can also synthesize unchanged entries when they
/// have external proof that the authoritative state did not change at a tick
/// (for example, a completed mutate tick with no component update).
#[derive(Component, Debug, Reflect)]
pub struct ConfirmedHistory<C> {
    buffer: VecDeque<(Tick, ConfirmedState<C>)>,
    /// True when the newest sample was synthesized by `push_unchanged`.
    ///
    /// Interpolation uses this to slide one unchanged anchor forward across
    /// consecutive completed mutate ticks instead of appending identical
    /// samples for every tick.
    ///
    /// For example, if the buffer is `[(A, V)]`, `push_unchanged(A + 1)`
    /// appends a synthesized sample and sets this flag:
    ///
    /// ```text
    /// [(A, V), (A + 1, V)]
    /// ```
    ///
    /// A later `push_unchanged(A + 2)` then moves that synthesized newest
    /// sample forward instead of appending another copy:
    ///
    /// ```text
    /// [(A, V), (A + 2, V)]
    /// ```
    ///
    /// If an out-of-order explicit update is later inserted before the
    /// start of that synthetic unchanged span, the synthesized sample remains
    /// valid:
    ///
    /// ```text
    /// [(A - 1, W), (A, V), (A + 2, V)]
    /// ```
    ///
    /// If an out-of-order explicit update is inserted at or after the start and
    /// before the synthesized newest sample, the synthesized sample is removed.
    /// Its value may no longer be the effective authoritative value for that
    /// tick:
    ///
    /// ```text
    /// [(A, V), (A + 1, W)]
    /// ```
    ///
    /// A later `push_unchanged(A + 2)` can then synthesize a fresh newest
    /// sample from `W`.
    newest_is_unchanged: bool,
    /// Start tick for the synthetic unchanged span when `newest_is_unchanged`.
    newest_unchanged_start: Option<Tick>,
}

impl<C> Default for ConfirmedHistory<C> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
            newest_is_unchanged: false,
            newest_unchanged_start: None,
        }
    }
}

// This matches the historical interpolation behavior: tests compare anchor
// ticks and the unchanged-anchor bit, not component values.
impl<C> PartialEq for ConfirmedHistory<C> {
    fn eq(&self, other: &Self) -> bool {
        let self_ticks: Vec<_> = self.buffer.iter().map(|(tick, _)| *tick).collect();
        let other_ticks: Vec<_> = other.buffer.iter().map(|(tick, _)| *tick).collect();
        self_ticks == other_ticks
            && self.newest_is_unchanged == other.newest_is_unchanged
            && self.newest_unchanged_start == other.newest_unchanged_start
    }
}

impl<C> ConfirmedHistory<C> {
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Oldest state in the buffer.
    pub fn oldest(&self) -> Option<&(Tick, ConfirmedState<C>)> {
        self.buffer.front()
    }

    /// Most recent state in the buffer.
    pub fn most_recent(&self) -> Option<&(Tick, ConfirmedState<C>)> {
        self.buffer.back()
    }

    #[doc(hidden)]
    pub fn buffer(&self) -> &VecDeque<(Tick, ConfirmedState<C>)> {
        &self.buffer
    }

    /// Get the n-th oldest tick in the buffer.
    pub fn get_nth_tick(&self, n: usize) -> Option<Tick> {
        self.buffer.get(n).map(|(tick, _)| *tick)
    }

    /// Get the n-th oldest state in the buffer.
    pub fn get_nth_state(&self, n: usize) -> Option<(Tick, &ConfirmedState<C>)> {
        self.buffer.get(n).map(|(tick, state)| (*tick, state))
    }

    /// The oldest present value in the history.
    pub fn start(&self) -> Option<(Tick, &C)> {
        self.get_nth(0)
    }

    /// The most recent present value in the history.
    pub fn newest(&self) -> Option<(Tick, &C)> {
        match self.buffer.back() {
            Some((tick, ConfirmedState::Confirmed(value))) => Some((*tick, value)),
            Some((_, ConfirmedState::Removed)) | None => None,
        }
    }

    /// Get the n-th oldest entry if it is a present value.
    pub fn get_nth(&self, n: usize) -> Option<(Tick, &C)> {
        match self.buffer.get(n) {
            Some((tick, ConfirmedState::Confirmed(value))) => Some((*tick, value)),
            Some((_, ConfirmedState::Removed)) | None => None,
        }
    }

    /// Get the latest present value at or before `tick`.
    pub fn get(&self, tick: Tick) -> Option<&C> {
        self.state_at_or_before(tick)
            .and_then(ConfirmedState::value)
    }

    /// Get the latest authoritative state at or before `tick`.
    pub fn state_at_or_before(&self, tick: Tick) -> Option<&ConfirmedState<C>> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        if partition == 0 {
            return None;
        }
        self.buffer.get(partition - 1).map(|(_, state)| state)
    }

    /// Get the authoritative state exactly at `tick`.
    pub fn get_state_at(&self, tick: Tick) -> Option<&ConfirmedState<C>> {
        let pos = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick < tick);
        self.buffer
            .get(pos)
            .and_then(|(buffer_tick, state)| (*buffer_tick == tick).then_some(state))
    }

    /// Insert an authoritative state while preserving tick order.
    pub fn insert(&mut self, tick: Tick, value: Option<C>) {
        let state = match value {
            Some(value) => ConfirmedState::Confirmed(value),
            None => ConfirmedState::Removed,
        };
        self.insert_state(tick, state);
    }

    /// Insert an authoritative state while preserving tick order.
    ///
    /// If this inserts an out-of-order explicit sample before the start of a
    /// newest sample synthesized by [`push_unchanged`], the synthetic newest
    /// sample remains valid. If it inserts at or after the synthetic span's
    /// start and before the synthetic newest sample, the synthetic newest
    /// sample is removed. A later unchanged-completeness tick can synthesize a
    /// fresh newest sample from the updated effective state.
    ///
    /// [`push_unchanged`]: ConfirmedHistory::push_unchanged
    pub fn insert_state(&mut self, tick: Tick, state: ConfirmedState<C>) {
        let preserve_synthetic_newest = self.should_preserve_synthetic_newest_for_insert(tick);
        self.invalidate_synthetic_newest_for_insert(tick);
        let pos = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick < tick);
        if pos < self.buffer.len() && self.buffer[pos].0 == tick {
            self.buffer[pos] = (tick, state);
        } else {
            self.buffer.insert(pos, (tick, state));
        }
        if !preserve_synthetic_newest {
            self.clear_synthetic_newest();
        }
    }

    fn should_preserve_synthetic_newest_for_insert(&self, tick: Tick) -> bool {
        self.newest_is_unchanged
            && self
                .newest_unchanged_start
                .is_some_and(|start_tick| tick < start_tick)
    }

    fn invalidate_synthetic_newest_for_insert(&mut self, tick: Tick) {
        if !self.newest_is_unchanged {
            return;
        }
        let Some(start_tick) = self.newest_unchanged_start else {
            self.clear_synthetic_newest();
            return;
        };
        let Some((newest_tick, _)) = self.buffer.back() else {
            self.clear_synthetic_newest();
            return;
        };
        if tick >= start_tick && tick < *newest_tick {
            self.buffer.pop_back();
            self.clear_synthetic_newest();
        }
    }

    fn clear_synthetic_newest(&mut self) {
        self.newest_is_unchanged = false;
        self.newest_unchanged_start = None;
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
    /// The caller must ensure `tick >= self.most_recent().map(|(tick, _)| *tick)`.
    /// Passing an older tick leaves the buffer out of order and breaks all
    /// lookup methods that rely on sorted ticks.
    ///
    /// [`insert`]: ConfirmedHistory::insert
    pub unsafe fn insert_assume_sorted(&mut self, tick: Tick, value: Option<C>) {
        let state = match value {
            Some(value) => ConfirmedState::Confirmed(value),
            None => ConfirmedState::Removed,
        };
        // SAFETY: This method's caller must uphold the sorted insertion precondition.
        unsafe { self.insert_state_assume_sorted(tick, state) };
    }

    /// Insert an authoritative state assuming `tick` is not older than the
    /// current newest sample.
    ///
    /// # Safety
    ///
    /// The caller must ensure `tick >= self.most_recent().map(|(tick, _)| *tick)`.
    /// Passing an older tick leaves the buffer out of order and breaks all
    /// lookup methods that rely on sorted ticks.
    pub unsafe fn insert_state_assume_sorted(&mut self, tick: Tick, state: ConfirmedState<C>) {
        debug_assert!(
            self.buffer
                .back()
                .is_none_or(|(newest_tick, _)| tick >= *newest_tick),
            "insert_state_assume_sorted called with out-of-order tick"
        );
        if let Some((last_tick, _)) = self.buffer.back()
            && *last_tick == tick
        {
            self.buffer.pop_back();
        }
        self.buffer.push_back((tick, state));
        self.clear_synthetic_newest();
    }

    /// Pop the oldest present value in the history.
    pub fn pop(&mut self) -> Option<(Tick, C)> {
        let popped = match self.buffer.pop_front() {
            Some((tick, ConfirmedState::Confirmed(value))) => Some((tick, value)),
            Some((_, ConfirmedState::Removed)) | None => None,
        };
        if let Some((popped_tick, _)) = popped.as_ref()
            && Some(*popped_tick) == self.newest_unchanged_start
        {
            self.clear_synthetic_newest();
        }
        if self.buffer.is_empty() {
            self.clear_synthetic_newest();
        }
        popped
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.clear_synthetic_newest();
    }

    /// Clear all states strictly older than `tick`.
    pub fn clear_until_tick(&mut self, tick: Tick) {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick < &tick);
        if partition > 0 {
            self.buffer.drain(0..partition);
        }
        if self
            .newest_unchanged_start
            .is_some_and(|start_tick| start_tick < tick)
        {
            self.clear_synthetic_newest();
        }
        if self.buffer.is_empty() {
            self.clear_synthetic_newest();
        }
    }

    /// Shift all stored ticks by `delta`.
    pub fn update_ticks(&mut self, delta: i32) {
        self.buffer.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
        if let Some(start_tick) = &mut self.newest_unchanged_start {
            *start_tick = *start_tick + delta;
        }
    }
}

impl<C: Clone> ConfirmedHistory<C> {
    /// Add a synthesized unchanged sample at `tick` by cloning the latest
    /// authoritative state at or before `tick`.
    ///
    /// Use this when another system has proven that the authoritative component
    /// state did not change at `tick` even though no explicit component update
    /// was received.
    pub fn add_unchanged(&mut self, tick: Tick) -> bool {
        let Some(state) = self.state_at_or_before(tick).cloned() else {
            return false;
        };
        if self.get_state_at(tick).is_some() {
            return false;
        }
        self.insert_state(tick, state);
        true
    }

    /// Advance the newest present value to `tick` for interpolation.
    ///
    /// Use this only when `tick` comes from a monotonic completeness signal,
    /// such as the latest completed mutate tick. Consecutive unchanged ticks
    /// update the same synthesized newest anchor instead of cloning the same
    /// value repeatedly.
    pub fn push_unchanged(&mut self, tick: Tick) -> Option<Tick> {
        let (newest_tick, newest_value) = self.newest()?;
        if tick <= newest_tick {
            return None;
        }

        if self.newest_is_unchanged {
            if let Some((most_recent_tick, _)) = self.buffer.back_mut() {
                *most_recent_tick = tick;
            }
        } else {
            self.buffer
                .push_back((tick, ConfirmedState::Confirmed(newest_value.clone())));
            self.newest_is_unchanged = true;
            self.newest_unchanged_start = Some(newest_tick);
        }
        Some(newest_tick)
    }

    /// Clear states older than `tick` while preserving the effective state at
    /// `tick` for future lookups.
    pub fn prune_before_preserving(&mut self, tick: Tick) -> Option<ConfirmedState<C>> {
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        if partition == 0 {
            return None;
        }
        self.buffer.drain(0..(partition - 1));
        let state = self.buffer.pop_front().map(|(_, state)| state)?;
        self.buffer.push_front((tick, state.clone()));
        self.clear_synthetic_newest();
        Some(state)
    }
}

/// The iterator contains the present values from oldest to most recent.
impl<'a, C> IntoIterator for &'a ConfirmedHistory<C> {
    type Item = (Tick, &'a C);
    type IntoIter = FilterMap<
        <&'a VecDeque<(Tick, ConfirmedState<C>)> as IntoIterator>::IntoIter,
        fn(&(Tick, ConfirmedState<C>)) -> Option<(Tick, &C)>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.buffer.iter().filter_map(|(tick, state)| match state {
            ConfirmedState::Confirmed(value) => Some((*tick, value)),
            ConfirmedState::Removed => None,
        })
    }
}

/// The iterator contains the present values from oldest to most recent.
impl<C> IntoIterator for ConfirmedHistory<C> {
    type Item = (Tick, C);
    type IntoIter = FilterMap<
        <VecDeque<(Tick, ConfirmedState<C>)> as IntoIterator>::IntoIter,
        fn((Tick, ConfirmedState<C>)) -> Option<(Tick, C)>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.buffer
            .into_iter()
            .filter_map(|(tick, state)| match state {
                ConfirmedState::Confirmed(value) => Some((tick, value)),
                ConfirmedState::Removed => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use test_log::test;

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    #[test]
    fn insert_supports_out_of_order_exact_samples() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(5), Some(TestValue(5.0)));
        history.insert(Tick(1), Some(TestValue(1.0)));
        history.insert(Tick(3), None);

        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(1), ConfirmedState::Confirmed(TestValue(1.0))),
                (Tick(3), ConfirmedState::Removed),
                (Tick(5), ConfirmedState::Confirmed(TestValue(5.0))),
            ])
        );
        assert_eq!(history.get(Tick(2)).unwrap().0, 1.0);
        assert!(history.get(Tick(3)).is_none());
        assert_eq!(history.get(Tick(5)).unwrap().0, 5.0);
    }

    #[test]
    fn add_unchanged_preserves_future_samples() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(2), Some(TestValue(2.0)));
        history.insert(Tick(10), Some(TestValue(10.0)));

        assert!(history.add_unchanged(Tick(5)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(5), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(10), ConfirmedState::Confirmed(TestValue(10.0))),
            ])
        );
    }

    #[test]
    fn push_unchanged_slides_newest_anchor() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(2), Some(TestValue(2.0)));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));
        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(8), ConfirmedState::Confirmed(TestValue(2.0))),
            ])
        );
    }

    #[test]
    fn out_of_order_insert_before_synthetic_span_preserves_synthetic_anchor() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(2), Some(TestValue(2.0)));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));

        history.insert(Tick(1), Some(TestValue(1.0)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(1), ConfirmedState::Confirmed(TestValue(1.0))),
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(5), ConfirmedState::Confirmed(TestValue(2.0))),
            ])
        );

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(1), ConfirmedState::Confirmed(TestValue(1.0))),
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(8), ConfirmedState::Confirmed(TestValue(2.0))),
            ])
        );
    }

    #[test]
    fn out_of_order_insert_inside_synthetic_span_invalidates_synthetic_anchor() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(2), Some(TestValue(2.0)));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));

        history.insert(Tick(3), Some(TestValue(3.0)));
        assert!(!history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, None);
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(3), ConfirmedState::Confirmed(TestValue(3.0))),
            ])
        );

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(3)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(3)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(3), ConfirmedState::Confirmed(TestValue(3.0))),
                (Tick(8), ConfirmedState::Confirmed(TestValue(3.0))),
            ])
        );
    }

    #[test]
    fn explicit_insert_at_synthetic_newest_replaces_synthetic_anchor() {
        let mut history = ConfirmedHistory::<TestValue>::default();
        history.insert(Tick(2), Some(TestValue(2.0)));

        assert_eq!(history.push_unchanged(Tick(5)), Some(Tick(2)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(2)));

        history.insert(Tick(5), Some(TestValue(5.0)));
        assert!(!history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, None);
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(5), ConfirmedState::Confirmed(TestValue(5.0))),
            ])
        );

        assert_eq!(history.push_unchanged(Tick(8)), Some(Tick(5)));
        assert!(history.newest_is_unchanged);
        assert_eq!(history.newest_unchanged_start, Some(Tick(5)));
        assert_eq!(
            history.buffer(),
            &VecDeque::from(vec![
                (Tick(2), ConfirmedState::Confirmed(TestValue(2.0))),
                (Tick(5), ConfirmedState::Confirmed(TestValue(5.0))),
                (Tick(8), ConfirmedState::Confirmed(TestValue(5.0))),
            ])
        );
    }
}
