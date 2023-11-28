//! # Time utilities
//!
//! ## Time Manager
//! This crate defines [`TimeManager`], which is responsible for keeping track of the time.
//! It will interact with bevy's [`Time`] resource, and potentially change the relative speed of the simulation.
//!
//! ## WrappedTime
//!
//! [`WrappedTime`] is a struct representing time, that wraps around 1 hour.
//! It contains some helper functions to compute the difference between two times.
use std::cmp::Ordering;
use std::fmt::Formatter;
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};
use std::time::Duration;

use bevy::prelude::{Time, Timer, TimerMode, Virtual};
use bitcode::{Decode, Encode};
use chrono::Duration as ChronoDuration;
use serde::{Deserialize, Serialize};

/// Time wraps after u32::MAX in microseconds (a bit over an hour)
pub const WRAPPING_TIME_US: u32 = u32::MAX;

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

    /// Update the relative speed of the simulation by updating bevy's Time resource
    pub fn update_relative_speed(&self, time: &mut Time<Virtual>) {
        time.set_relative_speed(self.base_relative_speed * self.sync_relative_speed)
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

    /// Current time since start, wrapped around 1 hour
    pub fn current_time(&self) -> WrappedTime {
        self.wrapped_time
    }
}

/// Time since start of server, in milliseconds
/// Serializes in a compact manner
/// Wraps around u32::max
#[derive(Default, Encode, Decode, Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
pub struct WrappedTime {
    // Amount of time elapsed since the start of the server, in microseconds
    // wraps around 1 hour
    // We use milli-seconds because micro-seconds lose precisions very quickly
    // #[bitcode_hint(expected_range = "0..3600000000")]
    pub(crate) elapsed_us_wrapped: u32,
}

impl std::fmt::Debug for WrappedTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WrappedTime")
            .field(
                "time",
                &Duration::from_micros(self.elapsed_us_wrapped as u64),
            )
            .finish()
    }
}

impl WrappedTime {
    pub fn new(elapsed_us_wrapped: u32) -> Self {
        Self { elapsed_us_wrapped }
    }

    pub fn from_duration(elapsed_wrapped: Duration) -> Self {
        // TODO: check cast?
        // I think this has wrapping behaviour
        let elapsed_us_wrapped = elapsed_wrapped.as_micros() as u32;
        Self { elapsed_us_wrapped }
    }

    pub fn to_duration(&self) -> Duration {
        Duration::from_micros(self.elapsed_us_wrapped as u64)
    }

    /// Returns time b - time a, in microseconds
    /// Can be positive if b is in the future, or negative is b is in the past
    pub fn wrapping_diff(a: &Self, b: &Self) -> i32 {
        const MAX: i32 = (WRAPPING_TIME_US / 2 - 1) as i32;
        const MIN: i32 = -MAX;
        const ADJUST: i32 = WRAPPING_TIME_US as i32;

        let a: i32 = a.elapsed_us_wrapped as i32;
        let b: i32 = b.elapsed_us_wrapped as i32;

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
        self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_sub(rhs_micros as u32);
    }
}

/// Returns the absolute duration between two times (no matter which one is ahead of which)!
/// Only valid for durations under 1 hour
impl SubAssign<ChronoDuration> for WrappedTime {
    fn sub_assign(&mut self, rhs: ChronoDuration) {
        let rhs_micros = rhs.num_microseconds().unwrap();
        // we can use wrapping_sub because we wrap around u32::max
        if rhs_micros > 0 {
            self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_sub(rhs_micros as u32);
        } else {
            self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_add(rhs_micros as u32);
        }
    }
}

impl Add<Duration> for WrappedTime {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self::Output {
        Self {
            elapsed_us_wrapped: self.elapsed_us_wrapped.wrapping_add(rhs.as_micros() as u32),
        }
    }
}

impl Add for WrappedTime {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            elapsed_us_wrapped: self.elapsed_us_wrapped.wrapping_add(rhs.elapsed_us_wrapped),
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
            self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_add(rhs_micros as u32);
        } else {
            self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_sub(rhs_micros as u32);
        }
    }
}

impl AddAssign<Duration> for WrappedTime {
    fn add_assign(&mut self, rhs: Duration) {
        self.elapsed_us_wrapped = self.elapsed_us_wrapped.wrapping_add(rhs.as_micros() as u32);
    }
}

impl Mul<f32> for WrappedTime {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self {
            elapsed_us_wrapped: ((self.elapsed_us_wrapped as f32) * rhs) as u32,
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

#[cfg(test)]
mod tests {
    use crate::shared::time_manager::WrappedTime;

    #[test]
    fn test_mul() {
        let a = WrappedTime::new(u32::MAX);
        let b = a * 2.0;
        assert_eq!(b.elapsed_us_wrapped, u32::MAX);
    }
}
