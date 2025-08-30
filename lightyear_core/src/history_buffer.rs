use crate::tick::Tick;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use bevy_ecs::component::Component;
use bevy_ecs::reflect::{ReflectComponent, ReflectResource};
use bevy_ecs::resource::Resource;
use bevy_reflect::Reflect;
use core::fmt::Debug;
use core::iter::FilterMap;
#[allow(unused_imports)]
use tracing::{debug, info};

/// Stores a past value in the history buffer
// We use this enum instead of Option<R> in case we need to distniguish between more states in the future
#[derive(Debug, PartialEq, Clone, Default, Reflect)]
pub enum HistoryState<R> {
    // we add a Default implementation simply so that Reflection works
    #[default]
    /// the value just got removed
    Removed,
    /// the value got updated
    Updated(R),
}

impl<'w, R> From<&'w HistoryState<R>> for Option<&'w R> {
    fn from(val: &'w HistoryState<R>) -> Self {
        match val {
            HistoryState::Removed => None,
            HistoryState::Updated(r) => Some(r),
        }
    }
}
impl<R> From<HistoryState<R>> for Option<R> {
    fn from(val: HistoryState<R>) -> Self {
        match val {
            HistoryState::Removed => None,
            HistoryState::Updated(r) => Some(r),
        }
    }
}

/// HistoryBuffer stores past values (usually of a Component or Resource) in a buffer, to allow for rollback
/// The values must always remain ordered from oldest (front) to most recent (back)
#[derive(Resource, Component, Debug, Reflect)]
#[reflect(Component, Resource)]
pub struct HistoryBuffer<R> {
    // Queue containing the history of the resource.
    // The front contains old elements, the back contains the more recent elements.
    // We will only store the history for the ticks where the resource got updated
    // (if the resource doesn't change, we don't store it)
    //
    // The ticks might become invalid in case of a TickEvent (the client tick is changed).
    // In that case we simply handle the TickEvent and update all the ticks inside this buffer.
    //
    // Another option would be to store the tick difference between two updates, and only the first (most recent update)
    // gets updated in case of a TickEvent.
    pub(crate) buffer: VecDeque<(Tick, HistoryState<R>)>,
}

impl<R> Default for HistoryBuffer<R> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
        }
    }
}

// This is mostly present for testing, we only compare the buffer ticks, not the values
impl<R> PartialEq for HistoryBuffer<R> {
    fn eq(&self, other: &Self) -> bool {
        let self_history: Vec<_> = self.buffer.iter().map(|(tick, _)| *tick).collect();
        let other_history: Vec<_> = other.buffer.iter().map(|(tick, _)| *tick).collect();
        self_history.eq(&other_history)
    }
}

impl<R> HistoryBuffer<R> {
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Oldest value in the buffer
    pub fn oldest(&self) -> Option<&(Tick, HistoryState<R>)> {
        self.buffer.front()
    }

    /// Most recent value in the buffer
    pub fn most_recent(&self) -> Option<&(Tick, HistoryState<R>)> {
        self.buffer.back()
    }

    #[doc(hidden)]
    /// Get the second most recent value in the buffer, knowing that tick is the current tick.
    ///
    /// Used to efficiently get the previous value when doing correction.
    pub fn second_most_recent(&self, tick: Tick) -> Option<&R> {
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

    /// Get the value at the specified tick.
    pub fn get(&self, tick: Tick) -> Option<&R> {
        // find first idx `partition` such that self.buffer[partition].0 > tick
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
        if partition == 0 {
            return None;
        }
        self.buffer.get(partition - 1).and_then(|(_, x)| x.into())
    }

    /// Reset the history for this resource
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Clear all the values in the history buffer that are older or equal than the specified tick
    pub fn clear_until_tick(&mut self, tick: Tick) {
        // self.buffer[partition] is the first element where the buffer_tick > tick
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        // all elements are strictly more recent than the tick
        if partition == 0 {
            return;
        }
        // remove all elements older than the tick
        self.buffer.drain(0..partition);
    }

    /// Add to the buffer that we received an update for the resource at the given tick
    /// The tick must be more recent than the most recent update in the buffer
    pub fn add_update(&mut self, tick: Tick, value: R) {
        self.add(tick, Some(value));
    }
    /// Add to the buffer that the value got removed at the given tick
    pub fn add_remove(&mut self, tick: Tick) {
        self.add(tick, None);
    }

    /// Add a value to the history buffer
    /// The tick must be strictly more recent than the most recent update in the buffer
    pub fn add(&mut self, tick: Tick, value: Option<R>) {
        if let Some(last_tick) = self.peek().map(|(tick, _)| tick) {
            // assert!(
            //     tick >= *last_tick,
            //     "Tick must be more recent than the last update in the buffer"
            // );
            if *last_tick == tick {
                debug!(
                    "Adding update to history buffer for tick: {:?} but it already had a value for that tick!",
                    tick
                );
                // in this case, let's pop back the update to replace it with the new value
                self.buffer.pop_back();
            }
        }
        self.buffer.push_back((
            tick,
            match value {
                Some(value) => HistoryState::Updated(value),
                None => HistoryState::Removed,
            },
        ));
    }

    /// Peek at the most recent value in the history buffer
    pub fn peek(&self) -> Option<&(Tick, HistoryState<R>)> {
        self.buffer.back()
    }

    /// In case of a TickEvent where the client tick is changed, we need to update the ticks in the buffer
    pub fn update_ticks(&mut self, delta: i16) {
        self.buffer.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
    }

    #[cfg(feature = "test_utils")]
    pub fn buffer(&self) -> &VecDeque<(Tick, HistoryState<R>)> {
        &self.buffer
    }
}

/// The iterator contains the elements that are actually present in the history
/// from the oldest to the most recent
impl<'a, R> IntoIterator for &'a HistoryBuffer<R> {
    type Item = (Tick, &'a R);
    type IntoIter = FilterMap<
        <&'a VecDeque<(Tick, HistoryState<R>)> as IntoIterator>::IntoIter,
        fn(&(Tick, HistoryState<R>)) -> Option<(Tick, &R)>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.buffer.iter().filter_map(|(tick, state)| match state {
            HistoryState::Updated(value) => Some((*tick, value)),
            HistoryState::Removed => None,
        })
    }
}

/// The iterator contains the elements that are actually present in the history
/// from the oldest to the most recent
impl<R> IntoIterator for HistoryBuffer<R> {
    type Item = (Tick, R);
    type IntoIter = FilterMap<
        <VecDeque<(Tick, HistoryState<R>)> as IntoIterator>::IntoIter,
        fn((Tick, HistoryState<R>)) -> Option<(Tick, R)>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.buffer
            .into_iter()
            .filter_map(|(tick, state)| match state {
                HistoryState::Updated(value) => Some((tick, value)),
                HistoryState::Removed => None,
            })
    }
}

impl<R: Clone> HistoryBuffer<R> {
    /// Clear the history of values strictly older than the specified tick,
    /// and return the value at the specified tick.
    ///
    /// CAREFUL:
    /// the history will only contain the ticks where the value got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks.
    /// (i.e. if the buffer contains values at tick 4 and 8. If we pop_until_tick(6), we cannot delete the value for tick 4
    /// because we still need in case we call pop_until_tick(7). What we'll do is remove the value for tick 4 and re-insert it
    /// for tick 6)
    pub fn pop_until_tick(&mut self, tick: Tick) -> Option<HistoryState<R>> {
        // self.buffer[partition] is the first element where the buffer_tick > tick
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        // all elements are strictly more recent than the tick
        if partition == 0 {
            return None;
        }
        // remove all elements strictly older than the tick. We need to keep the element at index `partition-1`
        // because that is the value at tick `tick`
        self.buffer.drain(0..(partition - 1));
        let res = self.buffer.pop_front().map(|(_, state)| state);

        // if there is a value, re-add the value at tick `tick` to the buffer, to make sure that we have a value for ticks
        // (tick + 1)..(self.buffer[partition].0)
        match res.as_ref() {
            None => {}
            Some(HistoryState::Removed) => {
                // TODO: is this necessary? we treat None and Removed the same way anyway
                self.buffer.push_front((tick, HistoryState::Removed))
            }
            Some(r) => self.buffer.push_front((tick, r.clone())),
        };
        res
    }

    /// Clear the history of all values apart from the value at `tick`, and return that value.
    ///
    /// We keep that value in the history because we could have several consecutive rollbacks to the same tick.
    ///
    /// CAREFUL:
    /// the history will only contain the ticks where the value got updated, and otherwise
    /// contains gaps. Therefore, we need to always leave a value in the history buffer so that we can
    /// get the values for the future ticks.
    /// (i.e. if the buffer contains values at tick 4 and 8. If we pop_until_tick(6), we cannot delete the value for tick 4
    /// because we still need in case we call pop_until_tick(7). What we'll do is remove the value for tick 4 and re-insert it
    /// for tick 6)
    pub fn clear_except_tick(&mut self, tick: Tick) -> Option<HistoryState<R>> {
        // self.buffer[partition] is the first element where the buffer_tick > tick
        let partition = self
            .buffer
            .partition_point(|(buffer_tick, _)| buffer_tick <= &tick);
        // all elements are strictly more recent than the tick
        if partition == 0 {
            self.clear();
            return None;
        }
        // remove all elements strictly older than the tick. We need to keep the element at index `partition-1`
        // because that is the value at tick `tick`
        self.buffer.drain(0..(partition - 1));
        let res = self.buffer.pop_front().map(|(_, state)| state);
        self.clear();

        // if there is a value, re-add the value at tick `tick` to the buffer, to make sure that we have a value for ticks
        // (tick + 1)..(self.buffer[partition].0)
        match res.as_ref() {
            None => {}
            Some(HistoryState::Removed) => {
                // TODO: is this necessary? we treat None and Removed the same way anyway
                self.buffer.push_front((tick, HistoryState::Removed))
            }
            Some(r) => self.buffer.push_front((tick, r.clone())),
        };
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use test_log::test;

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    /// Test adding and removing updates to the resource history
    #[test]
    fn test_add_remove_history() {
        let mut history = HistoryBuffer::<TestValue>::default();

        // check when we try to access a value when the buffer is empty
        assert_eq!(history.pop_until_tick(Tick(0)), None);

        // check when we try to access an exact tick
        history.add_update(Tick(1), TestValue(1.0));
        history.add_update(Tick(2), TestValue(2.0));
        assert_eq!(
            history.pop_until_tick(Tick(2)),
            Some(HistoryState::Updated(TestValue(2.0)))
        );
        // check that we cleared older ticks, and that the most recent value still remains
        assert_eq!(history.buffer.len(), 1);
        assert_eq!(
            history.buffer,
            VecDeque::from(vec![(Tick(2), HistoryState::Updated(TestValue(2.0)))])
        );

        // check when we try to access a value in-between ticks
        history.add_update(Tick(4), TestValue(4.0));
        // we retrieve the most recent value older or equal to Tick(3)
        assert_eq!(
            history.pop_until_tick(Tick(3)),
            Some(HistoryState::Updated(TestValue(2.0)))
        );
        assert_eq!(history.buffer.len(), 2);
        // check that the most recent value got added back to the buffer at the popped tick
        assert_eq!(
            history.buffer,
            VecDeque::from(vec![
                (Tick(3), HistoryState::Updated(TestValue(2.0))),
                (Tick(4), HistoryState::Updated(TestValue(4.0)))
            ])
        );

        // check that nothing happens when we try to access a value before any ticks
        assert_eq!(history.pop_until_tick(Tick(0)), None);
        assert_eq!(history.buffer.len(), 2);

        history.add_remove(Tick(5));
        assert_eq!(history.buffer.len(), 3);
        assert_eq!(history.peek(), Some(&(Tick(5), HistoryState::Removed)));

        history.clear_until_tick(Tick(3));
        assert_eq!(
            history.buffer,
            VecDeque::from(vec![
                (Tick(4), HistoryState::Updated(TestValue(4.0))),
                (Tick(5), HistoryState::Removed)
            ])
        );

        assert_eq!(
            history.into_iter().collect::<Vec<_>>(),
            vec![(Tick(4), TestValue(4.0))]
        );
    }

    #[test]
    fn test_get() {
        let mut history = HistoryBuffer::<TestValue>::default();

        history.add_update(Tick(1), TestValue(1.0));
        history.add_update(Tick(2), TestValue(2.0));
        history.add_update(Tick(4), TestValue(4.0));

        assert_eq!(history.get(Tick(0)), None);
        assert_eq!(history.get(Tick(1)), Some(&TestValue(1.0)));
        assert_eq!(history.get(Tick(2)), Some(&TestValue(2.0)));
        assert_eq!(history.get(Tick(3)), Some(&TestValue(2.0)));
        assert_eq!(history.get(Tick(4)), Some(&TestValue(4.0)));
        assert_eq!(history.get(Tick(5)), Some(&TestValue(4.0)));
    }

    #[test]
    fn test_second_most_recent() {
        let mut history = HistoryBuffer::<TestValue>::default();

        assert_eq!(history.second_most_recent(Tick(0)), None);
        history.add_update(Tick(1), TestValue(1.0));
        assert_eq!(history.second_most_recent(Tick(1)), None);
        history.add_update(Tick(2), TestValue(2.0));
        assert_eq!(history.second_most_recent(Tick(2)), Some(&TestValue(1.0)));
        assert_eq!(history.second_most_recent(Tick(3)), Some(&TestValue(2.0)));
        history.add_update(Tick(4), TestValue(4.0));
        assert_eq!(history.second_most_recent(Tick(4)), Some(&TestValue(2.0)));
        assert_eq!(history.second_most_recent(Tick(5)), Some(&TestValue(4.0)));
    }
}
