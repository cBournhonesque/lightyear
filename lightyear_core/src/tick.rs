use core::time::Duration;
use lightyear_utils::wrapping_id;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::resource::Resource;
use bevy_platform::sync::atomic::{AtomicU16, Ordering};
use bevy_reflect::Reflect;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

impl Ord for Tick {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for Tick {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Tick {
    /// Wrap-aware "is this tick more recent than `other`" check.
    /// Correct only when `|self - other| < 32768`. Use this instead of `>`
    /// when you want sequence semantics, not numeric ordering.
    #[inline]
    pub fn is_newer_than(self, other: Self) -> bool {
        lightyear_utils::wrapping_id::wrapping_diff(other.0, self.0) > 0
    }

    /// Wrap-aware "is this tick older than `other`".
    #[inline]
    pub fn is_older_than(self, other: Self) -> bool {
        lightyear_utils::wrapping_id::wrapping_diff(other.0, self.0) < 0
    }

    /// Wrap-aware Ordering for use in comparators (e.g., partition_point closures).
    #[inline]
    pub fn wrapping_cmp(&self, other: &Self) -> core::cmp::Ordering {
        use core::cmp::Ordering;
        match lightyear_utils::wrapping_id::wrapping_diff(other.0, self.0) {
            0 => Ordering::Equal,
            x if x > 0 => Ordering::Greater,
            _ => Ordering::Less,
        }
    }
}

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
    use alloc::vec::Vec;
    use std::thread;
    use test_log::test;

    #[test]
    fn tick_ord_transitivity() {
        let t1 = Tick(0u16);
        let t2 = Tick(32_768u16);
        let t3 = Tick(33_000u16);

        assert!(t1 < t2);
        assert!(t2 < t3);
        assert!(
            t1 < t3,
            "transitivity violated: t1 < t2 && t2 < t3 but !(t1 < t3)"
        );
        assert!(t3 > t1);
    }

    #[test]
    fn tick_ord_btreemap_consistency() {
        use std::collections::BTreeMap;

        let mut m: BTreeMap<Tick, u32> = BTreeMap::new();
        for t in [0u16, 16_000, 32_768, 33_000, 49_000] {
            m.insert(Tick(t), t as u32);
        }

        for t in [0u16, 16_000, 32_768, 33_000, 49_000] {
            assert!(
                m.get(&Tick(t)).is_some(),
                "BTreeMap.get failed for tick {}",
                t
            );
        }

        let visited: Vec<u16> = m.keys().map(|k| k.0).collect();
        assert_eq!(
            visited.len(),
            5,
            "BTreeMap iteration count wrong: {visited:?}"
        );
    }

    #[test]
    fn tick_ord_is_total() {
        // BTreeMap-style total ordering: numeric u16 ordering.
        assert!(Tick(0) < Tick(32_768));
        assert!(Tick(32_768) < Tick(33_000));
        assert!(Tick(0) < Tick(33_000));
        // Reflexivity / antisymmetry.
        assert!(Tick(100) <= Tick(100));
        assert!(Tick(100) >= Tick(100));
        assert!(Tick(50) < Tick(100));
        assert!(Tick(100) > Tick(50));
    }

    #[test]
    fn tick_wrap_aware_methods() {
        // Within the half-range where wrap-aware sequence comparison is defined.
        assert!(Tick(32_000).is_newer_than(Tick(0)));
        assert!(!Tick(0).is_newer_than(Tick(32_000)));
        // Across wrap boundary
        assert!(Tick(5).is_newer_than(Tick(65_530)));
        assert!(Tick(65_530).is_older_than(Tick(5)));
        assert!(!Tick(5).is_older_than(Tick(65_530)));
    }

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
