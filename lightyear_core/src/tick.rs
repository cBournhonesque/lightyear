use core::time::Duration;
use lightyear_utils::wrapping_id;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::resource::Resource;
use bevy_platform::sync::atomic::{AtomicU16, Ordering};
use bevy_reflect::Reflect;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

/// Resource that contains the global TickDuration
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Reflect, Deref, DerefMut)]
pub struct TickDuration(pub Duration);

#[derive(Debug, Default, Reflect)]
pub struct AtomicTick(pub AtomicU16);

impl From<u16> for AtomicTick {
    fn from(value: u16) -> Self {
        AtomicTick(AtomicU16::new(value))
    }
}

impl AtomicTick {
    /// Gets the current value of the tick.
    ///
    /// Uses Relaxed because only the final value (usually the minimum) matters.
    pub fn get(&self) -> Tick {
        Tick(self.0.load(Ordering::Relaxed))
    }

    /// Replicate the value of the AtomicU16 with the new tick value
    /// only if that value is lower than the current value.
    pub fn set_if_lower(&self, new_value: Tick) {
        let mut current = self.0.load(Ordering::Acquire);
        // Loop until we successfully update the value.
        loop {
            // If the new value isn't lower, there's nothing to do.
            if wrapping_id::wrapping_diff(current, new_value.0) >= 0 {
                break;
            }

            // Attempt to swap the `current` value with `new_value`.
            // This will only succeed if the atomic's value is still `current`.
            // If another thread changed it, `compare_exchange` will fail and
            // return the `Err` variant containing the now-current value.
            match self.0.compare_exchange(
                current,
                new_value.0,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                // Successfully swapped, we are done.
                Ok(_) => break,
                // The value was changed by another thread.
                // The loop will retry with the new current value.
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

    // TODO: test with loom?
    #[test]
    fn test_shared_atomic_tick_minimum() {
        // Initialize the counter with a high value.
        let min_value_tracker = AtomicTick::from(10);

        let values_to_test = vec![u16::MAX - 5, 5, 100, u16::MAX];
        let expected_minimum = Tick(u16::MAX - 5);

        // Spawn several threads, each trying to set a new minimum.
        let tracker_clone = &min_value_tracker;
        thread::scope(|s| {
            for val in values_to_test {
                s.spawn(move || {
                    tracker_clone.set_if_lower(Tick(val));
                });
            }
        });
        // The final value will be the lowest value from the `values_to_test` vector.
        assert_eq!(min_value_tracker.get(), expected_minimum);
    }
}
