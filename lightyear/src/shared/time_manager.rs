/*! Module to handle tracking time

# Time Manager
This crate defines [`TimeManager`], which is responsible for keeping track of the time.
It will interact with bevy's [`Time`] resource, and potentially change the relative speed of the simulation.

# WrappedTime
[`WrappedTime`] is a struct representing time.
The network serialization uses a u32 which can only represent times up to 46 days.
This module contains some helper functions to compute the difference between two times.
*/
use std::fmt::Formatter;
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};

use bevy::app::{App, RunFixedMainLoop};
use bevy::prelude::{IntoSystemConfigs, Plugin, Res, ResMut, Resource, Time, Timer, TimerMode};
use bevy::time::Fixed;
use bevy::utils::Duration;
use bevy::utils::Instant;
use chrono::Duration as ChronoDuration;
use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};
pub use wrapped_time::WrappedTime;

use crate::prelude::Tick;

// TODO: put this in networking plugin instead?
/// Run Condition to check if we are ready to send packets
pub(crate) fn is_ready_to_send(time_manager: Res<TimeManager>) -> bool {
    time_manager.is_ready_to_send()
}

/// Plugin that will centralize information about the various times (real, virtual, fixed)
/// as well as track when we should send updates to the remote
pub(crate) struct TimePlugin {
    /// Interval at which we send updates to the remote
    pub(crate) send_interval: Duration,
}

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.insert_resource(TimeManager::new(self.send_interval));
        // SYSTEMS
        app.add_systems(
            RunFixedMainLoop,
            update_overstep.after(bevy::time::run_fixed_main_schedule),
        );
    }
}

fn update_overstep(mut time_manager: ResMut<TimeManager>, fixed_time: Res<Time<Fixed>>) {
    time_manager.update_overstep(fixed_time.overstep_fraction());
}

#[derive(Resource)]
pub struct TimeManager {
    /// The virtual time
    wrapped_time: WrappedTime,
    /// The real time
    real_time: WrappedTime,
    /// The remaining time after running the fixed-update steps, as a fraction of the tick time
    overstep: f32,
    /// The time since the last frame; gets update by bevy's Time resource at the start of the frame
    delta: Duration,
    /// The relative speed set by the client.
    pub base_relative_speed: f32,
    /// Should we speedup or slowdown the simulation to sync the ticks?
    /// >1.0 = speedup, <1.0 = slowdown
    /// We speed up the virtual time so that our ticks go faster/slower
    /// Things that depend on real time (ping/pong times), channel/packet managers, send_interval should be unaffected
    pub(crate) sync_relative_speed: f32,
    /// Timer to keep track on we send the next update
    send_timer: Option<Timer>,
    /// Instant at the start of the frame
    frame_start: Option<Instant>,
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
            real_time: WrappedTime::new(0),
            overstep: 0.0,
            delta: Duration::default(),
            base_relative_speed: 1.0,
            sync_relative_speed: 1.0,
            send_timer,
            frame_start: None,
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

    /// Get the overstep (remaining time after running the fixed-update steps)
    /// as a fraction of the tick time
    pub fn overstep(&self) -> f32 {
        self.overstep
    }

    /// Get the relative speed at which the simulation should be running
    pub fn get_relative_speed(&self) -> f32 {
        self.base_relative_speed * self.sync_relative_speed
    }

    // pub fn update_real(&mut self, delta: Duration) {
    //     self.real_time.elapsed = Instant::now();
    // }

    /// Update the time by applying the latest delta
    /// delta: delta time since last frame
    /// overstep: remaining time after running the fixed-update steps
    pub(crate) fn update(&mut self, delta: Duration) {
        self.delta = delta;
        self.wrapped_time.elapsed += delta;
        self.frame_start = Some(Instant::now());
        if let Some(timer) = self.send_timer.as_mut() {
            timer.tick(delta);
        }
    }

    // TODO: reuse time-real for this?
    /// Compute the real time elapsed since the start of the frame
    /// (useful for
    pub(crate) fn real_time_since_frame_start(&self) -> Duration {
        self.frame_start
            .map(|start| Instant::now() - start)
            .unwrap_or_default()
    }

    /// Update the overstep (right after the overstep was computed, after RunFixedUpdateLoop)
    pub(crate) fn update_overstep(&mut self, overstep: f32) {
        self.overstep = overstep;
    }

    fn update_real(&mut self, real_delta: Duration) {
        self.real_time.elapsed += real_delta;
    }

    /// Current time since start, wrapped around 46 days
    pub fn current_time(&self) -> WrappedTime {
        self.wrapped_time
    }

    #[cfg(test)]
    pub(crate) fn set_current_time(&mut self, time: WrappedTime) {
        self.wrapped_time = time;
    }
}

mod wrapped_time {
    use anyhow::Context;

    use bitcode::encoding::{Encoding, Fixed};
    use bitcode::read::Read;
    use bitcode::write::Write;

    use crate::_reexport::{ReadBuffer, WriteBuffer};
    use crate::protocol::BitSerializable;

    use super::*;

    /// Time since start of server, in milliseconds
    /// Serializes in a compact manner (we only serialize up to the milliseconds)
    /// Valid only up to u32::MAX milliseconds (46 days)
    #[derive(Default, Copy, Clone, Eq, PartialEq, Debug, PartialOrd, Ord)]
    pub struct WrappedTime {
        pub(crate) elapsed: Duration,
    }

    impl BitSerializable for WrappedTime {
        fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
            writer
                .encode(self, Fixed)
                .context("error encoding WrappedTime")
        }

        fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            reader
                .decode::<Self>(Fixed)
                .context("error decoding WrappedTime")
        }
    }

    // NOTE: we only encode the milliseconds up to u32, which is 46 days
    impl Encode for WrappedTime {
        const ENCODE_MIN: usize = u32::ENCODE_MIN;
        const ENCODE_MAX: usize = u32::ENCODE_MAX;

        fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> bitcode::Result<()> {
            let millis: u32 = self.elapsed.as_millis().try_into().unwrap_or(u32::MAX);
            Encode::encode(&millis, encoding, writer)
        }
    }
    impl Decode for WrappedTime {
        const DECODE_MIN: usize = u32::DECODE_MIN;
        const DECODE_MAX: usize = u32::DECODE_MAX;

        fn decode(encoding: impl Encoding, reader: &mut impl Read) -> bitcode::Result<Self> {
            let millis: u32 = Decode::decode(encoding, reader)?;
            Ok(Self {
                elapsed: Duration::from_millis(millis as u64),
            })
        }
    }

    impl std::fmt::Display for WrappedTime {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            std::fmt::Debug::fmt(self, f)
        }
    }

    impl WrappedTime {
        pub fn new(elapsed_ms: u32) -> Self {
            Self {
                elapsed: Duration::from_millis(elapsed_ms as u64),
            }
        }

        /// Returns the number of milliseconds since the start of the server
        /// Saturates after 46 days
        pub fn millis(&self) -> u32 {
            self.elapsed.as_millis().try_into().unwrap_or(u32::MAX)
        }

        pub fn from_duration(elapsed: Duration) -> Self {
            // u128 as u32 wraps around u32::max, which is what we want
            // let elapsed_ms_wrapped = elapsed_wrapped.as_millis() as u32;
            Self { elapsed }
        }

        pub fn from_tick(tick: Tick, generation: u16, tick_duration: Duration) -> Self {
            let elapsed =
                ((generation as u32 * (u16::MAX as u32 + 1)) + tick.0 as u32) * tick_duration;
            Self { elapsed }
        }

        /// Convert the time to a tick, using the tick duration.
        pub fn to_tick(&self, tick_duration: Duration) -> Tick {
            Tick((self.elapsed.as_nanos() / tick_duration.as_nanos()) as u16)
        }

        /// If the time is between two ticks, give us the overstep as a percentage of a tick duration
        pub fn tick_overstep(&self, tick_duration: Duration) -> f32 {
            (self.elapsed.as_nanos() % tick_duration.as_nanos()) as f32
                / tick_duration.as_nanos() as f32
        }

        pub fn to_duration(&self) -> Duration {
            self.elapsed
        }

        // TODO: we use the time to compute the tick, but the problem is that time/tick could be not in sync?
        /// The wrapping 'generation' of the tick (by looking at what the corresponding time is)
        /// We use the fact that the period is a certain amount of time to be sure in cases
        /// where the tick doesn't match the time exactly
        pub fn tick_generation(&self, tick_duration: Duration, tick: Tick) -> u16 {
            let period = tick_duration * (u16::MAX as u32 + 1);
            // TODO: use try into instead of as, to avoid wrapping?
            let gen = (self.elapsed.as_nanos() / period.as_nanos()) as u16;
            let remainder =
                ((self.elapsed.as_nanos() % period.as_nanos()) / tick_duration.as_nanos()) as u16;

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

    /// Returns the absolute duration between two times (no matter which one is ahead of which)!
    impl Sub for WrappedTime {
        type Output = ChronoDuration;

        fn sub(self, rhs: Self) -> Self::Output {
            ChronoDuration::from_std(self.elapsed).unwrap()
                - ChronoDuration::from_std(rhs.elapsed).unwrap()
        }
    }

    impl Sub<Duration> for WrappedTime {
        type Output = WrappedTime;

        fn sub(self, rhs: Duration) -> Self::Output {
            Self {
                elapsed: self.elapsed.saturating_sub(rhs),
            }
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
            self.elapsed = self.elapsed.saturating_sub(rhs);
        }
    }

    /// Returns the absolute duration between two times (no matter which one is ahead of which)!
    /// Only valid for durations under 1 hour
    impl SubAssign<ChronoDuration> for WrappedTime {
        fn sub_assign(&mut self, rhs: ChronoDuration) {
            let rhs_micros = rhs.num_microseconds().unwrap();
            if rhs_micros > 0 {
                self.elapsed = self
                    .elapsed
                    .saturating_sub(Duration::from_micros(rhs_micros as u64));
            } else {
                self.elapsed += Duration::from_micros(-rhs_micros as u64);
            }
        }
    }

    impl Add<Duration> for WrappedTime {
        type Output = Self;
        fn add(self, rhs: Duration) -> Self::Output {
            Self {
                elapsed: self.elapsed + rhs,
            }
        }
    }

    impl Add for WrappedTime {
        type Output = Self;

        fn add(self, rhs: Self) -> Self::Output {
            Self {
                elapsed: self.elapsed + rhs.elapsed,
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
                self.elapsed += Duration::from_micros(rhs_micros as u64);
            } else {
                self.elapsed = self
                    .elapsed
                    .saturating_sub(Duration::from_micros(-rhs_micros as u64));
            }
        }
    }

    impl AddAssign<Duration> for WrappedTime {
        fn add_assign(&mut self, rhs: Duration) {
            self.elapsed += rhs;
        }
    }

    // NOTE: Mul doesn't work if multiplying creates a time that is more than 1 hour
    // This only works for small time differences
    impl Mul<f32> for WrappedTime {
        type Output = Self;

        fn mul(self, rhs: f32) -> Self::Output {
            Self {
                elapsed: self.elapsed.mul_f32(rhs),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mul() {
        let a = WrappedTime::new(u32::MAX);
        let b = a * 2.0;
        // TODO
        // assert_eq!(b.elapsed_ms_wrapped, u32::MAX);
    }

    #[test]
    fn test_sub() {
        let a = WrappedTime::new(0);
        let b = WrappedTime::new(1000);

        assert_eq!(b - a, chrono::Duration::milliseconds(1000));
        assert_eq!(a - b, chrono::Duration::milliseconds(-1000));
        assert_eq!(b - Duration::from_millis(2000), a);
        assert_eq!(b - ChronoDuration::milliseconds(2000), a);
        assert_eq!(b + ChronoDuration::milliseconds(-2000), a);

        // can represent a difference between two times as a negative chrono duration
        assert_eq!(
            b - WrappedTime::new(2000),
            ChronoDuration::milliseconds(-1000)
        );
    }

    #[test]
    fn test_add() {
        let a = WrappedTime::new(0);
        let b = WrappedTime::new(1000);

        assert_eq!(a + b, WrappedTime::new(1000));
        assert_eq!(b + Duration::from_millis(2000), WrappedTime::new(3000));
        assert_eq!(
            b + ChronoDuration::milliseconds(2000),
            WrappedTime::new(3000)
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
        let period = tick_duration * (u16::MAX as u32 + 1);
        let a = WrappedTime::new(0);
        assert_eq!(a.tick_generation(tick_duration, Tick(0)), 0);
        assert_eq!(a.tick_generation(tick_duration, Tick(10)), 0);

        // b's tick_from_time is tick 0 of gen 1
        let b = WrappedTime::from_duration(period);
        assert_eq!(b.tick_generation(tick_duration, Tick(0)), 1);
        assert_eq!(b.tick_generation(tick_duration, Tick(65000)), 0);

        // c's tick_from_time is tick 1 of gen 1
        let c = WrappedTime::from_duration(period + tick_duration);
        assert_eq!(c.tick_generation(tick_duration, Tick(1)), 1);
        assert_eq!(c.tick_generation(tick_duration, Tick(0)), 1);
        assert_eq!(c.tick_generation(tick_duration, Tick(65000)), 0);

        // d's tick_from_time is tick 65000 of gen 1
        let d = WrappedTime::from_duration(period + tick_duration * 65000);
        assert_eq!(d.tick_generation(tick_duration, Tick(64000)), 1);
        assert_eq!(d.tick_generation(tick_duration, Tick(65200)), 1);
        assert_eq!(d.tick_generation(tick_duration, Tick(0)), 2);
        assert_eq!(d.tick_generation(tick_duration, Tick(1)), 2);

        // e's tick is around 2300 of gen 0
        let e = WrappedTime::new(35120);
        assert_eq!(e.tick_generation(tick_duration, Tick(2247)), 0);
    }

    #[test]
    fn test_from_tick() {
        let tick_duration = Duration::from_secs_f32(1.0 / 64.0);
        assert_eq!(
            WrappedTime::from_tick(Tick(u16::MAX), 0, tick_duration),
            WrappedTime::from_duration(tick_duration * (u16::MAX as u32))
        );
        assert_eq!(
            WrappedTime::from_tick(Tick(0), 1, tick_duration),
            WrappedTime::from_duration(tick_duration * (u16::MAX as u32 + 1))
        );
        assert_eq!(
            WrappedTime::from_tick(Tick(1), 1, tick_duration),
            WrappedTime::from_duration(tick_duration * (u16::MAX as u32 + 2))
        );
    }

    #[test]
    fn test_to_tick() {
        let tick_duration = Duration::from_secs_f32(1.0 / 64.0);

        let time = WrappedTime::from_duration(tick_duration * (u16::MAX as u32));
        assert_eq!(time.to_tick(tick_duration), Tick(u16::MAX));

        let time = WrappedTime::from_duration(tick_duration * (u16::MAX as u32 + 1));
        assert_eq!(time.to_tick(tick_duration), Tick(0));

        let time = WrappedTime::from_duration(tick_duration * (u16::MAX as u32 + 2));
        assert_eq!(time.to_tick(tick_duration), Tick(1));
    }

    #[test]
    fn test_tick_overstep() {
        let tick_duration = Duration::from_secs_f32(1.0 / 64.0);

        let time = WrappedTime::from_duration(tick_duration.mul_f32(0.5));
        assert_eq!(time.tick_overstep(tick_duration), 0.5);

        let time = WrappedTime::from_duration(tick_duration.mul_f32(1.5));
        assert_eq!(time.tick_overstep(tick_duration), 0.5);

        let time = WrappedTime::from_duration(tick_duration.mul_f32(u16::MAX as f32 + 1.5));
        assert_eq!(time.tick_overstep(tick_duration), 0.5);
    }
}
