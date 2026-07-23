use core::ops::{Add, AddAssign, Deref as CoreDeref, Sub};
use core::time::Duration;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::resource::Resource;
use bevy_platform::sync::atomic::{AtomicU32, Ordering};
use bevy_reflect::Reflect;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};

/// Monotonically increasing simulation tick shared by the server and clients.
///
/// Unlike packet and message sequence IDs, ticks are numerically ordered and do not wrap. At
/// 60 Hz, exhausting the `u32` range takes more than two years of continuous runtime. Arithmetic
/// that produces another [`Tick`] saturates at `0` or [`u32::MAX`] instead of wrapping; signed
/// tick differences saturate to the `i32` range.
#[repr(transparent)]
#[derive(
    Serialize,
    Deserialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Ord,
    PartialOrd,
    Default,
    Reflect,
)]
pub struct Tick(pub u32);

impl ToBytes for Tick {
    fn bytes_len(&self) -> usize {
        core::mem::size_of::<u32>()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u32(self.0)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        Ok(Self(buffer.read_u32()?))
    }
}

impl From<u32> for Tick {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl CoreDeref for Tick {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Sub for Tick {
    type Output = i32;

    fn sub(self, rhs: Self) -> Self::Output {
        let difference = i64::from(self.0) - i64::from(rhs.0);
        difference.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
}

impl Sub<u32> for Tick {
    type Output = Self;

    fn sub(self, rhs: u32) -> Self::Output {
        Self(self.0.saturating_sub(rhs))
    }
}

impl Add for Tick {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign<u32> for Tick {
    fn add_assign(&mut self, rhs: u32) {
        self.0 = self.0.saturating_add(rhs);
    }
}

impl Add<i32> for Tick {
    type Output = Self;

    fn add(self, rhs: i32) -> Self::Output {
        Self(self.0.saturating_add_signed(rhs))
    }
}

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
    /// Uses the tick's ordinary numeric ordering.
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
    fn tick_uses_numeric_ordering_and_saturating_arithmetic() {
        assert!(Tick(u32::MAX) > Tick(0));
        assert_eq!(Tick(0) - 1, Tick(0));
        assert_eq!(Tick(u32::MAX) + 1, Tick(u32::MAX));
        assert_eq!(Tick(10) + (-20), Tick(0));
        assert_eq!(Tick(u32::MAX - 1) + Tick(2), Tick(u32::MAX));
        assert_eq!(Tick(u32::MAX) - Tick(0), i32::MAX);
        assert_eq!(Tick(0) - Tick(u32::MAX), i32::MIN);
    }

    #[test]
    fn test_shared_atomic_tick_minimum() {
        // Plain comparison: minimum is the numerically smallest value.
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
