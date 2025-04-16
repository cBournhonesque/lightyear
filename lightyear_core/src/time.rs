/*!
[`WrappedTime`] is a struct representing time.
The network serialization uses a u32 which can only represent times up to 46 days.
This module contains some helper functions to compute the difference between two times.
*/
use crate::tick::Tick;
use bevy::prelude::*;
use core::cmp::Ordering;
use core::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use core::time::Duration;
use lightyear_serde::reader::ReadInteger;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer
};

// TODO: maybe let the user choose between u8 or u16 for quantization?
// quantization error for u8 is about 0.2%, for u16 is 0.0008%
/// Overstep fraction towards the next tick
///
/// Represents a value between 0.0 and 1.0 that indicates progress towards the next tick
/// Serializes to a u8 value for network transmission
#[derive(Debug, Copy, Clone, Default, Reflect)]
pub struct Overstep {
    value: f32,
}

impl Overstep {
    pub fn new(value: f32) -> Self {
        // TODO: panic if value outside of bounds?
        Self { 
            value: value.clamp(0.0, 1.0)
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }
    
    pub fn from_f32(value: f32) -> Self {
        Self::new(value)
    }
    
    pub fn from_u8(value: u8) -> Self {
        Self::new(value as f32 / u8::MAX as f32)
    }
    
    pub fn to_u8(&self) -> u8 {
        (self.value * u8::MAX as f32).round() as u8
    }
}

impl PartialEq for Overstep {
    fn eq(&self, other: &Self) -> bool {
        // For exact equality, we compare the quantized values
        self.to_u8() == other.to_u8()
    }
}

impl Eq for Overstep {}

impl PartialOrd for Overstep {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Overstep {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.partial_cmp(&other.value).expect("NaN overstep is invalid")
    }
}

impl ToBytes for Overstep {
    fn bytes_len(&self) -> usize {
        1 // we only need 1 byte for a u8
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        Ok(buffer.write_u8(self.to_u8())?)
    }

    fn from_bytes(buffer: &mut lightyear_serde::reader::Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self::from_u8(buffer.read_u8()?))
    }
}


impl Add for Overstep {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.value + rhs.value)
    }
}

impl Sub for Overstep {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.value - rhs.value)
    }
}

impl AddAssign for Overstep {
    fn add_assign(&mut self, rhs: Self) {
        self.value = (self.value + rhs.value).clamp(0.0, 1.0);
    }
}

impl SubAssign for Overstep {
    fn sub_assign(&mut self, rhs: Self) {
        self.value = (self.value - rhs.value).clamp(0.0, 1.0);
    }
}

impl From<f32> for Overstep {
    fn from(value: f32) -> Self {
        Self::new(value)
    }
}

impl From<Overstep> for f32 {
    fn from(overstep: Overstep) -> Self {
        overstep.value
    }
}

// TODO: it would be nice if the tick duration was encoded in the tick itself
// TODO: maybe have a Tick trait with an associated constant TICK_DURATION
//  then the user can specify impl Tick<TICK_DURATION=16ms> for MyTick

// TODO: maybe have a constant TICK_DURATION as a generic, so we have Tick<T> around.

// TODO: maybe put this in lightyear_core?
/// Uniquely identify a instant across all timelines
#[derive(Default, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Reflect)]
pub struct TickInstant {
    pub tick: Tick,
    /// Overstep as a fraction towards the next tick
    pub overstep: Overstep,
}

impl TickInstant {
    /// Convert this instant to a duration
    pub fn as_duration(&self, tick_duration: Duration) -> Duration {
        tick_duration.mul_f32(self.tick.0 as f32 + self.overstep.value())
    }

    pub fn as_time_delta(&self, tick_duration: Duration) -> TimeDelta {
        let duration = self.as_duration(tick_duration);
        TimeDelta::from_duration(duration).expect("Duration should be valid")
    }

    /// Convert a duration to a TickInstant
    pub fn from_duration(duration: Duration, tick_duration: Duration) -> Self {
        let total_ticks = (duration.as_secs_f32() / tick_duration.as_secs_f32()).floor() as u16;
        let overstep = (duration.as_secs_f32() / tick_duration.as_secs_f32()) - total_ticks as f32;
        Self {
            tick: Tick(total_ticks),
            overstep: Overstep::from_f32(overstep),
        }
    }

    pub fn from_time_delta(delta: TimeDelta, tick_duration: Duration) -> Self {
        let duration = delta.as_duration().expect("Duration should be valid");
        Self::from_duration(duration, tick_duration)
    }
}


impl From<TickDelta> for TickInstant {
    fn from(value: TickDelta) -> Self {
        if value.is_negative() {
            panic!("Cannot convert negative TickDelta to TickInstant");
        }
        Self {
            tick: Tick(value.tick_diff),
            overstep: value.overstep,
        }
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

/// Represents the difference between two TickInstants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickDelta {
    tick_diff: u16,
    overstep: Overstep,
    /// True if the delta is negative
    neg: bool,
}

impl From<Tick> for TickDelta {
    fn from(value: Tick) -> Self {
        Self {
            tick_diff: value.0,
            overstep: Overstep::default(),
            neg: false,
        }
    }
}

impl From<PositiveTickDelta> for TickDelta {
    fn from(value: PositiveTickDelta) -> Self {
        Self {
            tick_diff: value.tick_diff,
            overstep: value.overstep,
            neg: false,
        }
    }
}

impl From<TickInstant> for TickDelta {
    fn from(value: TickInstant) -> Self {
        Self {
            tick_diff: value.tick.0,
            overstep: value.overstep,
            neg: false,
        }
    }
}

impl TickDelta {
    pub fn new(tick_diff: u16, overstep: Overstep, neg: bool) -> Self {
        Self {
            tick_diff,
            overstep,
            neg,
        }
    }

    pub fn is_positive(&self) -> bool {
        !self.neg
    }

    pub fn is_negative(&self) -> bool {
        self.neg
    }

    pub fn to_duration(&self, tick_duration: Duration) -> Duration {
        let total_ticks = self.tick_diff as f32 + self.overstep.value();
        tick_duration.mul_f32(total_ticks)
    }

    pub fn from_duration(duration: Duration, tick_duration: Duration) -> Self {
        let total_ticks_f = duration.as_secs_f32() / tick_duration.as_secs_f32();
        let tick_diff = total_ticks_f.floor() as u16;
        let overstep_value = total_ticks_f - tick_diff as f32;

        Self {
            tick_diff,
            overstep: Overstep::from_f32(overstep_value),
            neg: false,
        }
    }

    pub fn to_time_delta(&self, tick_duration: Duration) -> TimeDelta {
        let duration = tick_duration.mul_f32(self.tick_diff as f32 + self.overstep.value());
        if self.neg {
            // Handle negative duration conversion
            match TimeDelta::from_duration(duration) {
                Ok(delta) => -delta,
                Err(_) => panic!("Failed to convert duration to TimeDelta"),
            }
        } else {
            TimeDelta::from_duration(duration).expect("Duration should be valid")
        }
    }

    /// Apply a delta number of ticks with no overstep
    pub fn from_i16(delta: i16) -> Self {
        if delta < 0 {
            Self {
                tick_diff: (-delta) as u16,
                overstep: Overstep::default(),
                neg: true,
            }
        } else {
            Self {
                tick_diff: delta as u16,
                overstep: Overstep::default(),
                neg: false,
            }
        }
    }

    /// Returns the number of tick difference (positive or negative) that this TickDelta represents,
    /// rounding to the closes integer value
    pub fn to_i16(&self) -> i16 {
        if self.is_negative() {
            -(self.tick_diff as i16 + self.overstep.value().round() as i16)
        } else {
            self.tick_diff as i16 + self.overstep.value().round() as i16
        }
    }

    pub fn from_time_delta(mut delta: TimeDelta, tick_duration: Duration) -> Self {
        let is_negative = !delta.is_positive();
        if is_negative {
            delta = -delta;
        }

        // Work with absolute duration
        let duration = match delta.as_duration() {
            Ok(d) => d,
            Err(_) => panic!("Failed to convert TimeDelta to Duration"),
        };

        let total_ticks_f = duration.as_secs_f32() / tick_duration.as_secs_f32();
        let tick_diff = total_ticks_f.floor() as u16;
        let overstep_value = total_ticks_f - tick_diff as f32;

        Self {
            tick_diff,
            overstep: Overstep::from_f32(overstep_value),
            neg: is_negative,
        }
    }

    pub fn zero() -> Self {
        Self {
            tick_diff: 0,
            overstep: Overstep::default(),
            neg: false,
        }
    }
}

impl Neg for TickDelta {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            tick_diff: self.tick_diff,
            overstep: self.overstep,
            neg: !self.neg,
        }
    }
}


impl Add for TickDelta {
    type Output = TickDelta;

    fn add(self, rhs: Self) -> Self::Output {
        if self.is_negative() {
            return rhs - (-self);
        }

        let total_ticks = self.tick_diff + rhs.tick_diff;
        let new_overstep = self.overstep.value() + rhs.overstep.value();

        // Handle overstep overflow
        let additional_ticks = new_overstep.floor() as u16;
        let final_overstep = new_overstep - additional_ticks as f32;

        Self {
            tick_diff: total_ticks + additional_ticks,
            overstep: Overstep::from_f32(final_overstep),
            neg: false,
        }
    }
}

impl Sub for TickDelta {
    type Output = TickDelta;

    fn sub(self, rhs: Self) -> Self::Output {
        if self.is_negative() {
            return rhs + (-self);
        }

        let total_ticks = self.tick_diff.wrapping_sub(rhs.tick_diff);

        // Handle underflow in overstep subtraction
        if self.overstep.value() >= rhs.overstep.value() {
            // No underflow
            TickDelta {
                tick_diff: total_ticks,
                overstep: Overstep::from_f32(self.overstep.value() - rhs.overstep.value()),
                neg: false,
            }
        } else {
            // Underflow - need to borrow from tick
            TickDelta {
                tick_diff: total_ticks.wrapping_sub(1),
                overstep: Overstep::from_f32(1.0 + self.overstep.value() - rhs.overstep.value()),
                neg: false,
            }
        }
    }
}


impl Mul<f32> for TickDelta {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        let total_ticks = (self.tick_diff as f32 * rhs).floor() as u16;
        let remainder = (self.tick_diff as f32 * rhs) - total_ticks as f32;
        let new_overstep = remainder + self.overstep.value() * rhs;

        // Handle overstep overflow
        let additional_ticks = new_overstep.floor() as u16;
        let final_overstep = new_overstep - additional_ticks as f32;

        Self {
            tick_diff: total_ticks + additional_ticks,
            overstep: Overstep::from_f32(final_overstep),
            neg: self.neg,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PositiveTickDelta {
    tick_diff: u16,
    overstep: Overstep,
}

impl From<TickDelta> for PositiveTickDelta {
    fn from(value: TickDelta) -> Self {
        if value.is_negative() {
            panic!("Cannot convert negative TickDelta to PositiveTickDelta");
        }
        Self {
            tick_diff: value.tick_diff,
            overstep: value.overstep,
        }
    }
}

impl ToBytes for PositiveTickDelta {
    fn bytes_len(&self) -> usize {
        self.tick_diff.bytes_len() + self.overstep.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.tick_diff.to_bytes(buffer)?;
        self.overstep.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized
    {
        let tick_diff = u16::from_bytes(buffer)?;
        let overstep = Overstep::from_bytes(buffer)?;
        Ok(Self {
            tick_diff,
            overstep,
        })
    }
}


/// Delta between two instants
///
/// This is mostly useful because it can represent a positive or a negative duration.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct TimeDelta {
    duration: chrono::TimeDelta
}

impl TimeDelta {
    pub fn is_positive(&self) -> bool {
        self.duration.num_nanoseconds().unwrap_or(0) >= 0
    }

    /// We convert negative durations to their absolute value
    pub fn as_duration(&self) -> Result<Duration, chrono::OutOfRangeError> {
        self.duration.to_std()
    }

    pub fn from_duration(duration: Duration) -> Result<Self, chrono::OutOfRangeError> {
        Ok(Self {
            duration: chrono::TimeDelta::from_std(duration)?
        })
    }
}

impl Neg for TimeDelta {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            duration: -self.duration
        }
    }
}

impl Add<TickDelta> for TickInstant {
    type Output = TickInstant;

    fn add(self, rhs: TickDelta) -> Self::Output {
        if rhs.is_negative() {
            return self - (-rhs);
        }

        let new_overstep_value = self.overstep.value() + rhs.overstep.value();
        let additional_ticks = new_overstep_value.floor() as u16;
        let final_overstep = new_overstep_value - additional_ticks as f32;

        TickInstant {
            tick: Tick(self.tick.0.wrapping_add(rhs.tick_diff).wrapping_add(additional_ticks)),
            overstep: Overstep::from_f32(final_overstep),
        }
    }
}

impl Sub<TickDelta> for TickInstant {
    type Output = TickInstant;

    fn sub(self, rhs: TickDelta) -> Self::Output {
        if rhs.is_negative() {
            return self + (-rhs);
        }

        let total_ticks = rhs.tick_diff;

        // Handle underflow in overstep subtraction
        if self.overstep.value() >= rhs.overstep.value() {
            // No underflow
            TickInstant {
                tick: Tick(self.tick.0.wrapping_sub(total_ticks)),
                overstep: Overstep::from_f32(self.overstep.value() - rhs.overstep.value()),
            }
        } else {
            // Underflow - need to borrow from tick
            TickInstant {
                tick: Tick(self.tick.0.wrapping_sub(total_ticks + 1)),
                overstep: Overstep::from_f32(1.0 + self.overstep.value() - rhs.overstep.value()),
            }
        }
    }
}

impl Sub for TickInstant {
    type Output = TickDelta;

    fn sub(self, rhs: TickInstant) -> Self::Output {
        if self >= rhs {
            // self is later than or equal to rhs
            let tick_diff = self.tick.0.wrapping_sub(rhs.tick.0);

            if self.overstep >= rhs.overstep {
                // No underflow in overstep
                TickDelta {
                    tick_diff,
                    overstep: Overstep::from_f32(self.overstep.value() - rhs.overstep.value()),
                    neg: false,
                }
            } else {
                // Overstep underflow, borrow from tick
                TickDelta {
                    tick_diff: tick_diff - 1,
                    overstep: Overstep::from_f32(1.0 + self.overstep.value() - rhs.overstep.value()),
                    neg: false,
                }
            }
        } else {
            // self is earlier than rhs, result will be negative
            -(rhs - self)
        }
    }
}


/// Event that can be triggered to update the tick duration.
///
/// If the trigger is global, it will update:
/// - Time<Fixed>
/// - the various Timelines
///
/// The event can also be triggered for a specific target to update only the components of that target.
#[derive(Event)]
pub struct SetTickDuration(pub Duration);


pub struct TimePlugin;

impl TimePlugin {
    fn update_tick_duration(trigger: Trigger<SetTickDuration>, mut time: ResMut<Time<Fixed>>) {
        time.set_timestep(trigger.0);
    }
}

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::update_tick_duration);
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use core::time::Duration;

    #[test]
    fn test_overstep_quantization_error() {
        // Test that the round trip error is less than 1% for values from 0.0 to 1.0
        for i in 0..=10 {
            let original_value = i as f32 / 10.0;
            let overstep = Overstep::from_f32(original_value);
            let quantized = overstep.to_u8();
            let round_trip = Overstep::from_u8(quantized).value();

            assert_relative_eq!(round_trip, original_value, epsilon = 0.01);
        }
    }


    #[test]
    fn test_tickinstant_ordering() {
        let t1 = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.5) };
        let t2 = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.7) };
        let t3 = TickInstant { tick: Tick(11), overstep: Overstep::from_f32(0.2) };

        assert!(t1 < t2);
        assert!(t2 < t3);
        assert!(t1 < t3);

        assert_eq!(t1.cmp(&t1), Ordering::Equal);
        assert_eq!(t1.cmp(&t2), Ordering::Less);
        assert_eq!(t2.cmp(&t1), Ordering::Greater);
    }

    #[test]
    fn test_tickinstant_add_positive_tickdelta() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.3) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.2), false);

        let result = tick_instant + tick_delta;

        assert_eq!(result.tick, Tick(15));
        assert_relative_eq!(result.overstep.value, 0.5);
    }

    #[test]
    fn test_tickinstant_add_negative_tickdelta() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.3) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.2), true); // negative delta

        let result = tick_instant + tick_delta;

        assert_eq!(result.tick, Tick(5));
        assert_relative_eq!(result.overstep.value, 0.1);
    }

    #[test]
    fn test_tickinstant_add_with_overstep_overflow() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.7) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.6), false);

        let result = tick_instant + tick_delta;

        // 0.7 + 0.6 = 1.3, which is 1 tick + 0.3 overstep
        assert_eq!(result.tick, Tick(16));
        assert_relative_eq!(result.overstep.value, 0.3);
    }

    #[test]
    fn test_tickinstant_sub_positive_tickdelta() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.7) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.2), false);

        let result = tick_instant - tick_delta;

        assert_eq!(result.tick, Tick(5));
        assert_relative_eq!(result.overstep.value(), 0.5);
    }

    #[test]
    fn test_tickinstant_sub_negative_tickdelta() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.3) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.2), true); // negative delta

        let result = tick_instant - tick_delta;

        assert_eq!(result.tick, Tick(15));
        assert_relative_eq!(result.overstep.value(), 0.5);
    }

    #[test]
    fn test_tickinstant_sub_with_overstep_underflow() {
        let tick_instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.3) };
        let tick_delta = TickDelta::new(5, Overstep::from_f32(0.7), false);

        let result = tick_instant - tick_delta;

        // 0.3 - 0.7 = -0.4, which becomes 0.6 with borrowing from tick
        assert_eq!(result.tick, Tick(4));
        assert_relative_eq!(result.overstep.value(), 0.6);
    }

    #[test]
    fn test_tickinstant_sub_tickinstant() {
        let t1 = TickInstant { tick: Tick(15), overstep: Overstep::from_f32(0.7) };
        let t2 = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.3) };

        // t1 - t2 (positive result)
        let delta = t1 - t2;
        assert_eq!(delta.tick_diff, 5);
        assert_relative_eq!(delta.overstep.value(), 0.4);
        assert!(!delta.neg);

        // t2 - t1 (negative result)
        let delta = t2 - t1;
        assert_eq!(delta.tick_diff, 5);
        assert_relative_eq!(delta.overstep.value(), 0.4);
        assert!(delta.neg);
    }

    #[test]
    fn test_tickinstant_sub_tickinstant_with_overstep_underflow() {
        let t1 = TickInstant { tick: Tick(15), overstep: Overstep::from_f32(0.2) };
        let t2 = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.7) };

        // Need to borrow from tick
        let delta = t1 - t2;
        assert_eq!(delta.tick_diff, 4);
        assert_relative_eq!(delta.overstep.value(), 0.5);
        assert!(!delta.neg);
    }

    #[test]
    fn test_tickdelta_negation() {
        let delta = TickDelta::new(5, Overstep::from_f32(0.3), false);
        let negated = -delta;

        assert_eq!(negated.tick_diff, 5);
        assert_relative_eq!(delta.overstep.value(), 0.3);
        assert!(negated.neg);

        // Double negation should return to original
        let double_negated = -negated;
        assert_eq!(double_negated.tick_diff, 5);
        assert_relative_eq!(delta.overstep.value(), 0.3);
        assert!(!double_negated.neg);
    }

    #[test]
    fn test_tickinstant_multiplication() {
        let instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.5) };

        // Simple multiplication
        let result = instant * 2.0;
        assert_eq!(result.tick, Tick(21));  // 10*2 + floor(0.5*2) = 20 + 1 = 21
        assert_relative_eq!(result.overstep.value, 0.0);

        // Fractional multiplication
        let result = instant * 1.5;
        assert_eq!(result.tick, Tick(15));  // 10*1.5 = 15
        assert_relative_eq!(result.overstep.value, 0.75); // 0.5*1.5 = 0.75

        // Multiplication causing overstep overflow
        let instant = TickInstant { tick: Tick(10), overstep: Overstep::from_f32(0.8) };
        let result = instant * 1.5;
        assert_eq!(result.tick, Tick(16));  // 10*1.5 + floor(0.8*1.5) = 15 + 1 = 16
        assert_relative_eq!(result.overstep.value, 0.2); // 0.8*1.5 = 1.2, which is 1 tick + 0.2 overstep
    }

    #[test]
    fn test_tick_conversion_roundtrip() {
        let tick_duration = Duration::from_millis(100);
        let original = TickInstant { tick: Tick(15), overstep: Overstep::from_f32(0.4) };

        // Convert to duration and back
        let duration = original.as_duration(tick_duration);
        let roundtrip = TickInstant::from_duration(duration, tick_duration);

        // Allow for small floating point error in overstep
        assert_eq!(roundtrip.tick, original.tick);

        assert!((roundtrip.overstep.value() - original.overstep.value()).abs() < 0.01);
    }

    #[test]
    fn test_tickdelta_conversion_roundtrip() {
        let tick_duration = Duration::from_millis(100);

        // Test positive delta
        let original_delta = TickDelta::new(5, Overstep::from_f32(0.3), false);
        let time_delta = original_delta.to_time_delta(tick_duration);
        let roundtrip = TickDelta::from_time_delta(time_delta, tick_duration);

        assert_eq!(roundtrip.tick_diff, original_delta.tick_diff);
        assert!((roundtrip.overstep.value() - original_delta.overstep.value()).abs() < 0.01);
        assert_eq!(roundtrip.neg, original_delta.neg);

        // Test negative delta
        let original_delta = TickDelta::new(7, Overstep::from_f32(0.6), true);
        let time_delta = original_delta.to_time_delta(tick_duration);
        let roundtrip = TickDelta::from_time_delta(time_delta, tick_duration);

        assert_eq!(roundtrip.tick_diff, original_delta.tick_diff);
        assert!((roundtrip.overstep.value() - original_delta.overstep.value()).abs() < 0.01);
        assert_eq!(roundtrip.neg, original_delta.neg);
    }
}

