/*!
[`WrappedTime`] is a struct representing time.
The network serialization uses a u32 which can only represent times up to 46 days.
This module contains some helper functions to compute the difference between two times.
*/
use crate::tick::Tick;
use bevy::platform_support::time::Instant;
use bevy::prelude::*;
use chrono::Duration as ChronoDuration;
use core::cmp::Ordering;
use core::fmt::Formatter;
use core::ops::{Add, AddAssign, Mul, Sub, SubAssign};

use core::time::Duration;

use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};

/// Overstep fraction towards the next tick, as 1/u16::MAX
///
/// We use this instead of f32 because:
/// - we don't need the level of precision of f32 and this saves 2 bytes on each packet
/// - f32 doesn't implement PartialEq or Eq
pub type Overstep = u16;

// TODO: it would be nice if the tick duration was encoded in the tick itself
// TODO: maybe have a Tick trait with an associated constant TICK_DURATION
//  then the user can specify impl Tick<TICK_DURATION=16ms> for MyTick

// TODO: maybe have a constant TICK_DURATION as a generic, so we have Tick<T> around.

// TODO: maybe put this in lightyear_core?
/// Uniquely identify a instant across all timelines
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct TickInstant {
    pub tick: Tick,
    /// Overstep as a fraction towards the next tick
    pub overstep: Overstep,
}

impl Mul<f32> for TickInstant {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        todo!()
    }
}


impl TickInstant {
    fn as_duration(&self, tick_duration: Duration) -> Duration {
        todo!()
    }
}

impl From<Tick> for TickInstant {
    fn from(value: Tick) -> Self {
        Self {
            tick: value,
            overstep: Overstep::default(),
        }
    }
}


/// Duration between two TickInstant
///
/// This is mostly useful because it can represent a positive or a negative duration.
/// We could also convert TickInstant to chrono::Duration to achieve the same result.
#[derive(Debug, PartialEq, Eq)]
pub struct TickDuration {
    tick: i16,
    overstep: Overstep
}

impl From<Tick> for TickDuration {
    fn from(value: Tick) -> Self {
        Self {
            // TODO: check that there is no overflow here! how do we cast correctly with panic?
            tick: value.0 as i16,
            overstep: Overstep::default()
        }
    }
}

impl TickDuration {
    pub fn is_positive(&self) -> bool {
        self.tick >= 0
    }

    /// Returns the amount of time represented by the [`TickDuration`] as a [`Duration`]
    /// (independently from whether the TickDuration is positive or not)
    pub fn as_duration(&self, tick_duration: Duration) -> Duration {
        todo!()
    }

    pub fn from_duration(duration: Duration, tick_duration: Duration) -> Self {
        todo!()
    }
}

impl PartialOrd for TickDuration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        todo!()
    }
}

impl Add<TickDuration> for TickDuration {
    type Output = Self;

    fn add(self, rhs: TickDuration) -> Self::Output {
        todo!()
    }
}



// TODO: maybe this is done
impl Ord for TickInstant {
    fn cmp(&self, other: &Self) -> Ordering {
        todo!()
        // TODO: basically lexicographical order, tick first and then overstep
        // match self.tick.cmp(&other.tick) {
        //     Ordering::Less => {}
        //     Ordering::Equal => {
        //
        //     }
        //     Ordering::Greater => {}
        // }
        // match wrapping_diff(self.tick.0, other.tick) {
        //     0 => {
        //
        //     },
        //     x if x > 0 => Ordering::Less,
        //     x if x < 0 => Ordering::Greater,
        //     _ => unreachable!(),
        // }
    }
}

impl PartialOrd for TickInstant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Sub<TickInstant> for TickInstant {
    type Output = TickDuration;

    fn sub(self, rhs: TickInstant) -> Self::Output {
        // check which TickInstant is bigger
        todo!()
    }
}

impl Add<TickInstant> for TickInstant {
    type Output = TickInstant;

    fn add(self, rhs: TickInstant) -> Self::Output {
        todo!()
    }
}

impl Add<TickDuration> for TickInstant {
    type Output = TickInstant;

    fn add(self, rhs: TickDuration) -> Self::Output {
        todo!()
    }
}

impl Sub<TickDuration> for TickInstant {
    type Output = TickInstant;

    fn sub(self, rhs: TickDuration) -> Self::Output {
        todo!()
    }
}






#[derive(Resource, Debug, PartialEq, Clone)]
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
    /// \>1.0 = speedup, <1.0 = slowdown
    /// We speed up the virtual time so that our ticks go faster/slower
    /// Things that depend on real time (ping/pong times), channel/packet managers, send_interval should be unaffected
    pub(crate) sync_relative_speed: f32,
    /// Instant at the start of the frame
    frame_start: Option<Instant>,
}

impl Default for TimeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeManager {
    pub fn new() -> Self {
        Self {
            wrapped_time: WrappedTime::new(0),
            real_time: WrappedTime::new(0),
            overstep: 0.0,
            delta: Duration::default(),
            base_relative_speed: 1.0,
            sync_relative_speed: 1.0,
            frame_start: None,
        }
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

    /// Update the time by applying the latest delta
    /// delta: delta time since last frame
    /// overstep: remaining time after running the fixed-update steps
    pub fn update(&mut self, delta: Duration) {
        self.delta = delta;
        self.wrapped_time.elapsed += delta;
        self.frame_start = Some(Instant::now());
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

    // TODO: some functions that now rely on this time should instead use the real time
    //  (channel retries, etc.)
    /// Current time since start, wrapped around 46 days
    /// This time doesn't get modified by TickEvents (re-syncs of client time to server time)
    ///
    /// You can access the WrappedTime that corresponds to the current tick using the
    /// SyncManager's `current_prediction_time` method
    pub fn current_time(&self) -> WrappedTime {
        self.wrapped_time
    }

    #[cfg(test)]
    pub(crate) fn set_current_time(&mut self, time: WrappedTime) {
        self.wrapped_time = time;
    }
}

/// Time since start of server, in milliseconds
/// Serializes in a compact manner (we only serialize up to the milliseconds)
/// Valid only up to u32::MAX milliseconds (46 days)
#[derive(Default, Copy, Clone, Eq, PartialEq, Debug, PartialOrd, Ord)]
pub struct WrappedTime {
    pub(crate) elapsed: Duration,
}

impl Serialize for WrappedTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.millis())
    }
}
struct WrappedTimeVisitor;
impl Visitor<'_> for WrappedTimeVisitor {
    type Value = WrappedTime;

    fn expecting(&self, formatter: &mut Formatter) -> core::fmt::Result {
        formatter.write_str("a u32 representing the time in milliseconds")
    }

    fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(WrappedTime::new(v))
    }
}
impl<'de> Deserialize<'de> for WrappedTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_u32(WrappedTimeVisitor)
    }
}

impl ToBytes for WrappedTime {
    fn bytes_len(&self) -> usize {
        4
    }

    // NOTE: we only encode the milliseconds up to u32, which is 46 days
    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u32(self.millis())?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let millis = buffer.read_u32()?;
        Ok(Self {
            elapsed: Duration::from_millis(millis as u64),
        })
    }
}

impl core::fmt::Display for WrappedTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
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

    // TODO: switch to f16?
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
        let generation = (self.elapsed.as_nanos() / period.as_nanos()) as u16;
        let remainder =
            ((self.elapsed.as_nanos() % period.as_nanos()) / tick_duration.as_nanos()) as u16;

        let tick_from_time = remainder as i32;
        let tick_from_tick = tick.0 as i32;
        // case 1: tick |G| tick_from_time
        if tick_from_time - tick_from_tick > i16::MAX as i32 {
            generation.saturating_add(1)
        // case 2: tick_from_time |G| tick
        } else if tick_from_time - tick_from_tick < i16::MIN as i32 {
            generation.saturating_sub(1)
        // case 3: |G| tick_from_time tick |G+1|
        } else {
            generation
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
