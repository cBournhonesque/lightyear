use bitcode::{Decode, Encode};
use std::cmp::Ordering;
use std::ops::{AddAssign, Sub};
use std::time::Duration;

pub const WRAPPING_TIME_MS: u32 = 4194304; // 2^22

/// Time since start of server, in milliseconds
/// Serializes in a compact manner
#[derive(Encode, Decode, Copy, Clone, Debug, Eq, PartialEq)]
pub struct WrappedTime {
    // Amount of time elapsed since the start of the server, in milliseconds
    // wraps around 1 hour
    #[bitcode_hint(expected_range = "0..4194304")]
    elapsed_ms_wrapped: u32,
}

impl WrappedTime {
    pub fn new(elapsed_ms_wrapped: u32) -> Self {
        Self { elapsed_ms_wrapped }
    }

    pub fn from_duration(elapsed_wrapped: Duration) -> Self {
        // TODO: check cast?
        let elapsed_ms_wrapped = elapsed_wrapped.as_millis() as u32;
        Self { elapsed_ms_wrapped }
    }

    pub fn to_duration(&self) -> Duration {
        Duration::from_millis(self.elapsed_ms_wrapped as u64)
    }

    pub fn elapsed_milliseconds(&self) -> u32 {
        self.elapsed_ms_wrapped
    }

    /// Returns time b - time a, in milliseconds
    /// Can be positive if b is in the future, or negative is b is in the past
    pub fn wrapping_diff(a: &Self, b: &Self) -> i32 {
        const MAX: i32 = 2097151; // 2^21 - 1
        const MIN: i32 = -2097151;
        const ADJUST: i32 = WRAPPING_TIME_MS as i32; // 2^22

        let a: i32 = a.elapsed_ms_wrapped as i32;
        let b: i32 = b.elapsed_ms_wrapped as i32;

        let mut result = b - a;
        if (MIN..=MAX).contains(&result) {
            result
        } else if b > a {
            result = b - (a + ADJUST);
            if (MIN..=MAX).contains(&result) {
                result
            } else {
                panic!("integer overflow, this shouldn't happen")
            }
        } else {
            result = (b + ADJUST) - a;
            if (MIN..=MAX).contains(&result) {
                result
            } else {
                panic!("integer overflow, this shouldn't happen")
            }
        }
    }
}

impl Ord for WrappedTime {
    fn cmp(&self, other: &Self) -> Ordering {
        match Self::wrapping_diff(self, other) {
            0 => Ordering::Equal,
            x if x > 0 => Ordering::Less,
            x if x < 0 => Ordering::Greater,
            _ => unreachable!(),
        }
    }
}

impl PartialOrd for WrappedTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Returns the absolute duration between two times (no matter which one is ahead of which)!
impl Sub for WrappedTime {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        let diff_ms = Self::wrapping_diff(&rhs, &self);
        Duration::from_millis(diff_ms.wrapping_abs() as u64)
    }
}

impl AddAssign<Duration> for WrappedTime {
    fn add_assign(&mut self, rhs: Duration) {
        let add_millis = rhs.as_millis() as u32;
        self.elapsed_ms_wrapped = (self.elapsed_ms_wrapped + add_millis) % WRAPPING_TIME_MS;
    }
}

impl From<Duration> for WrappedTime {
    fn from(value: Duration) -> Self {
        Self::from_duration(value)
    }
}

impl From<WrappedTime> for Duration {
    fn from(value: WrappedTime) -> Self {
        value.to_duration()
    }
}
