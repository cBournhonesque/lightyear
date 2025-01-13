use crate::prelude::Tick;
use bevy::prelude::{Component, Reflect, Resource};
use bevy::prelude::{ReflectComponent, ReflectResource};
use std::collections::VecDeque;

/// Stores a past value in the history buffer
#[derive(Debug, PartialEq, Clone, Default, Reflect)]
pub(crate) enum HistoryState<R> {
    // we add a Default implementation simply so that Reflection works
    #[default]
    /// the value just got removed
    Removed,
    /// the value got updated
    Updated(R),
}

/// HistoryBuffer stores past values (usually of a Component or Resource) in a buffer, to allow for rollback
/// The values must always remain ordered from oldest (front) to most recent (back)
#[derive(Resource, Component, Debug, Reflect)]
#[reflect(Component, Resource)]
pub(crate) struct HistoryBuffer<R> {
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
    /// Reset the history for this resource
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Add to the buffer that we received an update for the resource at the given tick
    /// The tick must be more recent than the most recent update in the buffer
    pub(crate) fn add_update(&mut self, tick: Tick, value: R) {
        self.add(tick, Some(value));
    }
    /// Add to the buffer that the value got removed at the given tick
    pub(crate) fn add_remove(&mut self, tick: Tick) {
        self.add(tick, None);
    }

    /// Add a value to the history buffer
    pub(crate) fn add(&mut self, tick: Tick, value: Option<R>) {
        self.buffer.push_back((
            tick,
            match value {
                Some(value) => HistoryState::Updated(value),
                None => HistoryState::Removed,
            },
        ));
    }

    /// Peek at the most recent value in the history buffer
    pub(crate) fn peek(&self) -> Option<&(Tick, HistoryState<R>)> {
        self.buffer.back()
    }

    /// In case of a TickEvent where the client tick is changed, we need to update the ticks in the buffer
    pub(crate) fn update_ticks(&mut self, delta: i16) {
        self.buffer.iter_mut().for_each(|(tick, _)| {
            *tick = *tick + delta;
        });
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
    /// (i.e. if the buffer contains values at tick 4 and 8. If we pop_until_tick(6),
    /// we still need to include for tick 4 in the buffer, so that if we call pop_until_tick(7) we still have the correct value
    pub(crate) fn pop_until_tick(&mut self, tick: Tick) -> Option<HistoryState<R>> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct TestValue(f32);

    /// Test adding and removing updates to the resource history
    #[test]
    fn test_add_remove_history() {
        let mut resource_history = HistoryBuffer::<TestValue>::default();

        // check when we try to access a value when the buffer is empty
        assert_eq!(resource_history.pop_until_tick(Tick(0)), None);

        // check when we try to access an exact tick
        resource_history.add_update(Tick(1), TestValue(1.0));
        resource_history.add_update(Tick(2), TestValue(2.0));
        assert_eq!(
            resource_history.pop_until_tick(Tick(2)),
            Some(HistoryState::Updated(TestValue(2.0)))
        );
        // check that we cleared older ticks, and that the most recent value still remains
        assert_eq!(resource_history.buffer.len(), 1);
        assert_eq!(
            resource_history.buffer,
            VecDeque::from(vec![(Tick(2), HistoryState::Updated(TestValue(2.0)))])
        );

        // check when we try to access a value in-between ticks
        resource_history.add_update(Tick(4), TestValue(4.0));
        // we retrieve the most recent value older or equal to Tick(3)
        assert_eq!(
            resource_history.pop_until_tick(Tick(3)),
            Some(HistoryState::Updated(TestValue(2.0)))
        );
        assert_eq!(resource_history.buffer.len(), 2);
        // check that the most recent value got added back to the buffer at the popped tick
        assert_eq!(
            resource_history.buffer,
            VecDeque::from(vec![
                (Tick(3), HistoryState::Updated(TestValue(2.0))),
                (Tick(4), HistoryState::Updated(TestValue(4.0)))
            ])
        );

        // check that nothing happens when we try to access a value before any ticks
        assert_eq!(resource_history.pop_until_tick(Tick(0)), None);
        assert_eq!(resource_history.buffer.len(), 2);

        resource_history.add_remove(Tick(5));
        assert_eq!(resource_history.buffer.len(), 3);
        assert_eq!(
            resource_history.peek(),
            Some(&(Tick(5), HistoryState::Removed))
        );

        resource_history.clear();
        assert_eq!(resource_history.buffer.len(), 0);
    }
}
