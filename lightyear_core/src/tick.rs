use core::time::Duration;
use lightyear_utils::wrapping_id;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::resource::Resource;
use bevy_platform::sync::atomic::{AtomicU32, Ordering};
use bevy_reflect::Reflect;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

/// Resource that contains the global TickDuration
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Reflect, Deref, DerefMut)]
pub struct TickDuration(pub Duration);

#[derive(Debug, Default, Reflect)]
pub struct AtomicTick(pub AtomicU32);

impl From<u32> for AtomicTick {
    fn from(value: u32) -> Self {
        AtomicTick(AtomicU32::new(value))
    }
}

impl AtomicTick {
    /// Initialize the tick to the maximum value.
    ///
    /// This is useful for trackers that compute the minimum tick across multiple
    /// sources via [`set_if_lower`](Self::set_if_lower): starting from the max
    /// ensures the first recorded value always wins.
    pub fn new_max() -> Self {
        AtomicTick(AtomicU32::new(u32::MAX))
    }

    /// Gets the current value of the tick.
    pub fn get(&self) -> Tick {
        Tick(self.0.load(Ordering::Relaxed))
    }

    /// Update the value only if the new tick is strictly lower than the current value.
    ///
    /// Uses plain (non-wrapping) comparison: with u32 ticks, wrapping never occurs
    /// during a game session (~828 days at 60 Hz), so a simple `<` is correct.
    pub fn set_if_lower(&self, new_value: Tick) {
        let mut current = self.0.load(Ordering::Acquire);
        loop {
            if new_value.0 >= current {
                break;
            }
            match self.0.compare_exchange(
                current,
                new_value.0,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(newly_read_value) => current = newly_read_value,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use std::thread;
    use test_log::test;

    #[test]
    fn test_shared_atomic_tick_minimum() {
        // Plain comparison: minimum is the numerically smallest value.
        // With u32 ticks, wrapping never occurs in practice.
        let min_value_tracker = AtomicTick::from(10u32);

        let values_to_test = vec![u32::MAX - 5, 5, 100, u32::MAX];
        let expected_minimum = Tick(5);

        let tracker_clone = &min_value_tracker;
        thread::scope(|s| {
            for val in values_to_test {
                s.spawn(move || {
                    tracker_clone.set_if_lower(Tick(val));
                });
            }
        });
        assert_eq!(min_value_tracker.get(), expected_minimum);
    }

    #[test]
    fn test_new_max_allows_any_tick_to_win() {
        // An AtomicTick initialized to MAX must accept any subsequent value
        // as "lower". This is the intended usage for minimum trackers.
        let tracker = AtomicTick::new_max();
        assert_eq!(tracker.get(), Tick(u32::MAX));

        tracker.set_if_lower(Tick(483));
        assert_eq!(tracker.get(), Tick(483));

        tracker.set_if_lower(Tick(200));
        assert_eq!(tracker.get(), Tick(200));

        // Higher tick should NOT update
        tracker.set_if_lower(Tick(300));
        assert_eq!(tracker.get(), Tick(200));
    }
}
