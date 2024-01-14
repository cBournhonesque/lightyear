/*! Module to handle tracking time

# Time Manager
This crate defines [`TimeManager`], which is responsible for keeping track of the time.
It will interact with bevy's [`Time`] resource, and potentially change the relative speed of the simulation.

# WrappedTime
[`WrappedTime`] is a struct representing time, that wraps around 1 hour.
It contains some helper functions to compute the difference between two times.
*/
use std::cmp::Ordering;
use std::fmt::Formatter;
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};
use std::time::Duration;

use crate::prelude::Tick;
use bevy::prelude::{Res, Resource, Time, Timer, TimerMode};
use chrono::Duration as ChronoDuration;
use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};

/// Time wraps after u32::MAX in milliseconds (a bit over 46 days)
pub const WRAPPING_TIME_US: u32 = u32::MAX;

/// Run Condition to check if we are ready to send packets
pub(crate) fn is_ready_to_send(time_manager: Res<TimeManager>) -> bool {
    time_manager.is_ready_to_send()
}

#[derive(Resource)]
pub struct TimeManager {
    /// The current time
    wrapped_time: WrappedTime,
    /// The remaining time after running the fixed-update steps
    overstep: Duration,
    /// The time since the last frame; gets update by bevy's Time resource at the start of the frame
    delta: Duration,
    /// The relative speed set by the client.
    pub base_relative_speed: f32,
    /// Should we speedup or slowdown the simulation to sync the ticks?
    /// >1.0 = speedup, <1.0 = slowdown
    pub(crate) sync_relative_speed: f32,
    /// Timer to keep track on we send the next update
    send_timer: Option<Timer>,
}

impl TimeManager {
    pub fn new(send_interval: Duration) -> Self {
        let send_timer = if send_interval == Duration::default() {
            None
        } else {
            Some(Timer::new(send_interval, TimerMode::Repeating))
        };
        Self {
            wrapped_time: WrappedTime::new(0),
            overstep: Duration::default(),
            delta: Duration::default(),
            base_relative_speed: 1.0,
            sync_relative_speed: 1.0,
            send_timer,
        }
    }

    pub(crate) fn is_ready_to_send(&self) -> bool {
        self.send_timer
            .as_ref()
            .map_or(true, |timer| timer.finished())
    }

    pub fn delta(&self) -> Duration {
        self.delta
    }

    pub fn overstep(&self) -> Duration {
        self.overstep
    }

    /// Get the relative speed at which the simulation should be running
    pub fn get_relative_speed(&self) -> f32 {
        self.base_relative_speed * self.sync_relative_speed
    }

    /// Update the time by applying the latest delta
    /// delta: delta time since last frame
    /// overstep: remaining time after running the fixed-update steps
    pub fn update(&mut self, delta: Duration, overstep: Duration) {
        self.delta = delta;
        self.wrapped_time += delta;
        // set the overstep to the overstep of fixed_time
        self.overstep = overstep;
        if let Some(timer) = self.send_timer.as_mut() {
            timer.tick(delta);
        }
    }

    /// Current time since start, wrapped around 46 days
    pub fn current_time(&self) -> WrappedTime {
        self.wrapped_time
    }
}

mod wrapped_time {
    use super::*;
    /// Time since start of server, in microseconds
    /// Serializes in a compact manner
    /// Wraps around u32::max
    #[derive(Default, Encode, Decode, Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
    pub struct WrappedTime {
        // Amount of time elapsed since the start of the server, in milliseconds
        // wraps around 46 days
        // #[bitcode_hint(expected_range = "0..3600000000")]
        pub(crate) elapsed_ms_wrapped: u32,
    }

    impl std::fmt::Debug for WrappedTime {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("WrappedTime")
                .field(
                    "time",
                    &Duration::from_micros(self.elapsed_ms_wrapped as u64),
                )
                .finish()
        }
    }

    impl WrappedTime {
        pub fn new(elapsed_us_wrapped: u32) -> Self {
            Self {
                elapsed_ms_wrapped: elapsed_us_wrapped,
            }
        }

        pub fn from_duration(elapsed_wrapped: Duration) -> Self {
            // u128 as u32 wraps around u32::max, which is what we want
            let elapsed_us_wrapped = elapsed_wrapped.as_micros() as u32;
            Self {
                elapsed_ms_wrapped: elapsed_us_wrapped,
            }
        }

        pub fn from_tick(tick: Tick, generation: u16, tick_duration: Duration) -> Self {
            let elapsed_ms_wrapped = ((generation as u32 * u16::MAX as u32 + 1) + tick.0 as u32)
                * tick_duration.as_millis() as u32;
            Self { elapsed_ms_wrapped }
        }

        pub fn to_duration(&self) -> Duration {
            Duration::from_micros(self.elapsed_ms_wrapped as u64)
        }

        /// Returns time b - time a, in microseconds
        /// Can be positive if b is in the future, or negative is b is in the past
        pub fn wrapping_diff(a: &Self, b: &Self) -> i32 {
            // const MAX: i64 = (WRAPPING_TIME_US / 2) as i64;
            const MAX: i64 = i32::MAX as i64;
            const MIN: i64 = i32::MIN as i64;
            const ADJUST: i64 = WRAPPING_TIME_US as i64 + 1;

            let a: i64 = a.elapsed_ms_wrapped as i64;
            let b: i64 = b.elapsed_ms_wrapped as i64;

            let mut result = b - a;
            if (MIN..=MAX).contains(&result) {
                result as i32
            } else if b > a {
                result = b - (a + ADJUST);
                if (MIN..=MAX).contains(&result) {
                    result as i32
                } else {
                    panic!("integer overflow, this shouldn't happen")
                }
            } else {
                result = (b + ADJUST) - a;
                if (MIN..=MAX).contains(&result) {
                    result as i32
                } else {
                    panic!("integer overflow, this shouldn't happen")
                }
            }
        }

        /// The wrapping 'generation' of the tick (by looking at what the corresponding time is)
        /// We use the fact that the period is a certain amount of time to be sure in cases
        /// where the tick doesn't match the time exactly
        pub fn tick_generation(&self, tick_duration: Duration, tick: Tick) -> u16 {
            let period = (u16::MAX as u32 + 1) * tick_duration.as_millis() as u32;
            let gen = (self.elapsed_ms_wrapped / period) as u16;
            let remainder = (self.elapsed_ms_wrapped % period) as u16;

            let tick_from_time = remainder as i32;
            let tick_from_tick = tick.0 as i32;
            // case 1: tick |G| tick_from_time
            if tick_from_time - tick_from_tick > i16::MAX as i32 {
                gen.saturating_add(1)
            // case 2: tick_from_time |G| tick
            } else if tick_from_time - tick_from_tick < i16::MIN as i32 {
                gen.saturating_sub(1)
            // case 3: |G| tick_from_time tick |G+1|
            } else {
                gen
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
        type Output = ChronoDuration;

        fn sub(self, rhs: Self) -> Self::Output {
            let diff_us = Self::wrapping_diff(&rhs, &self);
            ChronoDuration::microseconds(diff_us as i64)
        }
    }

    impl Sub<Duration> for WrappedTime {
        type Output = WrappedTime;

        fn sub(self, rhs: Duration) -> Self::Output {
            let mut result = self;
            result -= rhs;
            result
        }
    }

    impl Sub<ChronoDuration> for WrappedTime {
        type Output = WrappedTime;

        fn sub(self, rhs: ChronoDuration) -> Self::Output {
            let mut result = self;
            result -= rhs;
            result
        }
    }

    /// Returns the absolute duration between two times (no matter which one is ahead of which)!
    /// Only valid for durations under 1 hour
    impl SubAssign<Duration> for WrappedTime {
        fn sub_assign(&mut self, rhs: Duration) {
            let rhs_micros = rhs.as_micros();
            // we can use wrapping_sub because we wrap around u32::max
            self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_sub(rhs_micros as u32);
        }
    }

    /// Returns the absolute duration between two times (no matter which one is ahead of which)!
    /// Only valid for durations under 1 hour
    impl SubAssign<ChronoDuration> for WrappedTime {
        fn sub_assign(&mut self, rhs: ChronoDuration) {
            let rhs_micros = rhs.num_microseconds().unwrap();
            // we can use wrapping_sub because we wrap around u32::max
            if rhs_micros > 0 {
                self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_sub(rhs_micros as u32);
            } else {
                self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_add(-rhs_micros as u32);
            }
        }
    }

    impl Add<Duration> for WrappedTime {
        type Output = Self;
        fn add(self, rhs: Duration) -> Self::Output {
            Self {
                elapsed_ms_wrapped: self.elapsed_ms_wrapped.wrapping_add(rhs.as_micros() as u32),
            }
        }
    }

    impl Add for WrappedTime {
        type Output = Self;

        fn add(self, rhs: Self) -> Self::Output {
            Self {
                elapsed_ms_wrapped: self.elapsed_ms_wrapped.wrapping_add(rhs.elapsed_ms_wrapped),
            }
        }
    }

    impl Add<ChronoDuration> for WrappedTime {
        type Output = Self;

        fn add(self, rhs: ChronoDuration) -> Self::Output {
            let mut result = self;
            result += rhs;
            result
        }
    }

    impl AddAssign<ChronoDuration> for WrappedTime {
        fn add_assign(&mut self, rhs: ChronoDuration) {
            let rhs_micros = rhs.num_microseconds().unwrap();
            if rhs_micros > 0 {
                self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_add(rhs_micros as u32);
            } else {
                self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_sub(-rhs_micros as u32);
            }
        }
    }

    impl AddAssign<Duration> for WrappedTime {
        fn add_assign(&mut self, rhs: Duration) {
            self.elapsed_ms_wrapped = self.elapsed_ms_wrapped.wrapping_add(rhs.as_micros() as u32);
        }
    }

    // NOTE: Mul doesn't work if multiplying creates a time that is more than 1 hour
    // This only works for small time differences
    impl Mul<f32> for WrappedTime {
        type Output = Self;

        fn mul(self, rhs: f32) -> Self::Output {
            Self {
                elapsed_ms_wrapped: ((self.elapsed_ms_wrapped as f32) * rhs) as u32,
            }
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
}

pub use wrapped_time::WrappedTime;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mul() {
        let a = WrappedTime::new(u32::MAX);
        let b = a * 2.0;
        assert_eq!(b.elapsed_ms_wrapped, u32::MAX);
    }

    #[test]
    fn test_wrapping() {
        let a = WrappedTime::new(u32::MAX);
        let b = WrappedTime::new(0);
        // the mid-way point is u32::MAX / 2
        let d = WrappedTime::new(u32::MAX / 2);
        let e = WrappedTime::new(u32::MAX / 2 + 1);
        let f = WrappedTime::new(u32::MAX / 2 + 10);
        assert_eq!(b - a, chrono::Duration::milliseconds(1));
        assert_eq!(a - b, chrono::Duration::milliseconds(-1));
        assert_eq!(d - b, chrono::Duration::milliseconds((u32::MAX / 2) as i64));
        assert_eq!(
            b - d,
            chrono::Duration::milliseconds(-((u32::MAX / 2) as i64))
        );
        assert_eq!(
            e - b,
            chrono::Duration::milliseconds(-((u32::MAX / 2 + 1) as i64))
        );
        assert_eq!(
            f - b,
            chrono::Duration::milliseconds(-((u32::MAX / 2 - 8) as i64))
        );
    }

    #[test]
    fn test_chrono_duration() {
        let a = WrappedTime::new(0);
        let b = WrappedTime::new(1000);
        let diff = b - a;
        assert_eq!(diff, chrono::Duration::milliseconds(1000));
        assert_eq!(a - b, chrono::Duration::milliseconds(-1000));
        assert_eq!(b + chrono::Duration::milliseconds(-1000), a);
        assert_eq!(a - chrono::Duration::milliseconds(-1000), b);

        assert_eq!(a + diff, b);

        assert_eq!(b - diff, a);
    }

    #[test]
    fn test_tick_generation() {
        let tick_duration = Duration::from_secs_f32(1.0 / 64.0);
        let tick_duration_ms = tick_duration.as_millis() as u32;
        let period = (u16::MAX as u32 + 1) * tick_duration_ms;
        let a = WrappedTime::new(0);
        assert_eq!(a.tick_generation(tick_duration, Tick(0)), 0);
        assert_eq!(a.tick_generation(tick_duration, Tick(10)), 0);

        // b's tick_from_time is tick 0 of gen 1
        let b = WrappedTime {
            elapsed_ms_wrapped: period,
        };
        assert_eq!(b.tick_generation(tick_duration, Tick(0)), 1);
        assert_eq!(b.tick_generation(tick_duration, Tick(65000)), 0);

        // c's tick_from_time is tick 1 of gen 1
        let c = WrappedTime {
            elapsed_ms_wrapped: period + tick_duration_ms,
        };
        assert_eq!(c.tick_generation(tick_duration, Tick(1)), 1);
        assert_eq!(c.tick_generation(tick_duration, Tick(0)), 1);
        assert_eq!(c.tick_generation(tick_duration, Tick(65000)), 0);

        // d's tick_from_time is tick 65000 of gen 1
        let d = WrappedTime {
            elapsed_ms_wrapped: period + 65000 * tick_duration_ms,
        };
        assert_eq!(d.tick_generation(tick_duration, Tick(64000)), 1);
        assert_eq!(d.tick_generation(tick_duration, Tick(65200)), 1);
        assert_eq!(d.tick_generation(tick_duration, Tick(0)), 2);
        assert_eq!(d.tick_generation(tick_duration, Tick(1)), 2);
    }
}
