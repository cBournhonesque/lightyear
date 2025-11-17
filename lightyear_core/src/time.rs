/*!
This module contains some helper functions to compute the difference between two times.
*/
use crate::tick::Tick;
use core::cmp::Ordering;
use core::fmt::{Debug, Formatter};
use core::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use bevy_reflect::Reflect;
use core::time::Duration;
use fixed::traits::ToFixed;
use fixed::types::{I16F16, U0F8, U0F16, U16F16};
use lightyear_serde::reader::ReadInteger;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};

#[cfg(any(not(feature = "test_utils"), feature = "not_mock"))]
pub use bevy_platform::time::Instant;
// We use global instead of a thread_local, because otherwise we would need to advance the Instant on all threads
#[cfg(all(feature = "test_utils", not(feature = "not_mock")))]
pub use mock_instant::global::Instant;

// TODO: maybe let the user choose between u8 or u16 for quantization?
// quantization error for u8 is about 0.2%, for u16 is 0.0008%
/// Overstep fraction towards the next tick
///
/// Represents a value between 0.0 and 1.0 that indicates progress towards the next tick
/// Serializes to a u8 value for network transmission
#[derive(Serialize, Deserialize, Debug, Copy, Clone, Default, Reflect)]
#[reflect(opaque)]
pub struct Overstep {
    value: U0F16,
}

impl Overstep {
    pub fn new(value: U0F16) -> Self {
        Self { value }
    }
    pub const fn lit(src: &str) -> Self {
        Self {
            value: U0F16::lit(src),
        }
    }

    pub fn value(&self) -> U0F16 {
        self.value
    }

    pub fn from_f32(value: f32) -> Self {
        Self::new(U0F16::saturating_from_num(value))
    }

    pub fn to_f32(&self) -> f32 {
        self.value.into()
    }

    pub fn from_u8(value: u8) -> Self {
        Self::new(U0F8::from_bits(value).into())
    }

    pub fn to_u8(&self) -> u8 {
        self.value.to_num::<U0F8>().to_bits()
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
        self.value
            .partial_cmp(&other.value)
            .expect("NaN overstep is invalid")
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
        self.value = self.value.saturating_add(rhs.value);
    }
}

impl SubAssign for Overstep {
    fn sub_assign(&mut self, rhs: Self) {
        self.value = self.value.saturating_sub(rhs.value);
    }
}

impl From<f32> for Overstep {
    fn from(value: f32) -> Self {
        Self::from_f32(value)
    }
}

impl From<Overstep> for f32 {
    fn from(overstep: Overstep) -> Self {
        overstep.to_f32()
    }
}

// TODO: it would be nice if the tick duration was encoded in the tick itself
// TODO: maybe have a Tick trait with an associated constant TICK_DURATION
//  then the user can specify impl Tick<TICK_DURATION=16ms> for MyTick

// TODO: maybe have a constant TICK_DURATION as a generic, so we have Tick<T> around.

// TODO: maybe put this in lightyear_core?
/// Uniquely identify a instant across all timelines
#[derive(Default, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Reflect)]
#[reflect(opaque)]
pub struct TickInstant {
    pub value: U16F16,
}

impl Debug for TickInstant {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl TickInstant {
    pub const fn lit(src: &str) -> Self {
        Self {
            value: U16F16::lit(src),
        }
    }
    pub const fn zero() -> Self {
        Self {
            value: U16F16::ZERO,
        }
    }
    pub fn tick(&self) -> Tick {
        Tick(self.value.to_num())
    }
    /// Overstep as a fraction towards the next tick
    pub fn overstep(&self) -> Overstep {
        Overstep::new(self.value.wrapping_to_fixed())
    }

    /// Construct a [`TickInstant`] from an integer tick and a fractional overstep.
    ///
    /// `tick` is the whole tick count, and `overstep` is the fraction towards
    /// the next tick in the range [0, 1).
    pub fn from_tick_and_overstep(tick: Tick, overstep: Overstep) -> Self {
        let base: U16F16 = tick.0.into();
        let frac: U16F16 = overstep.value().into();
        Self { value: base + frac }
    }

    /// Convert this instant to a duration
    pub fn as_duration(&self, tick_duration: Duration) -> Duration {
        tick_duration.mul_f32(self.value.to_num())
    }

    pub fn as_time_delta(&self, tick_duration: Duration) -> TimeDelta {
        let duration = self.as_duration(tick_duration);
        TimeDelta::from_duration(duration).expect("Duration should be valid")
    }

    /// Convert a duration to a TickInstant
    pub fn from_duration(duration: Duration, tick_duration: Duration) -> Self {
        let ticks_f32 = duration.as_secs_f32() / tick_duration.as_secs_f32();
        Self {
            value: ticks_f32.wrapping_to_fixed(),
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
            value: value.value.cast_unsigned(),
        }
    }
}

impl From<Tick> for TickInstant {
    fn from(value: Tick) -> Self {
        Self {
            value: value.0.into(),
        }
    }
}

/// Represents the difference between two TickInstants
#[derive(Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(opaque)]
pub struct TickDelta {
    /// This is the combined representation of a signed 16-bit tick diff (range -32,768 to 32,767)
    /// plus an unsigned 16-bit overstep (range 0 to ~0.99998, always positive).
    value: I16F16,
}

impl Debug for TickDelta {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:.6}", self.value)
    }
}

impl From<Tick> for TickDelta {
    fn from(value: Tick) -> Self {
        Self {
            value: value.0.cast_signed().into(),
        }
    }
}

impl From<i16> for TickDelta {
    fn from(value: i16) -> Self {
        Self {
            value: value.into(),
        }
    }
}

impl From<PositiveTickDelta> for TickDelta {
    fn from(value: PositiveTickDelta) -> Self {
        Self {
            value: value.value.wrapping_to_fixed(),
        }
    }
}

impl From<TickInstant> for TickDelta {
    fn from(value: TickInstant) -> Self {
        Self {
            value: value.value.cast_signed(),
        }
    }
}

impl TickDelta {
    pub fn new(value: I16F16) -> Self {
        Self { value }
    }
    pub const fn lit(src: &str) -> Self {
        Self {
            value: I16F16::lit(src),
        }
    }

    pub fn tick_diff(&self) -> u16 {
        self.value.unsigned_abs().to_num::<u16>()
    }
    pub fn overstep(&self) -> Overstep {
        Overstep::new(self.value.unsigned_abs().wrapping_to_num())
    }

    pub fn is_positive(&self) -> bool {
        self.value.is_positive()
    }

    pub fn is_negative(&self) -> bool {
        self.value.is_negative()
    }

    pub fn to_duration(&self, tick_duration: Duration) -> Duration {
        tick_duration.mul_f32(self.value.to_num())
    }

    pub fn from_duration(duration: Duration, tick_duration: Duration) -> Self {
        debug_assert!(
            tick_duration > Duration::ZERO,
            "Tick duration must be positive"
        );
        let ticks_f32 = duration.as_secs_f32() / tick_duration.as_secs_f32();
        Self {
            value: ticks_f32.wrapping_to_fixed(),
        }
    }

    pub fn to_time_delta(&self, tick_duration: Duration) -> TimeDelta {
        let tick_duration_f32 = tick_duration.as_secs_f32();
        let duration = tick_duration_f32 * self.value.to_num::<f32>();
        if self.is_negative() {
            // Handle negative duration conversion
            match TimeDelta::from_duration(Duration::from_secs_f32(-duration)) {
                Ok(delta) => -delta,
                Err(_) => panic!("Failed to convert duration to TimeDelta"),
            }
        } else {
            TimeDelta::from_duration(Duration::from_secs_f32(duration))
                .expect("Duration should be valid")
        }
    }

    pub fn to_f32(&self) -> f32 {
        self.value.to_num()
    }

    /// Apply a delta number of ticks with no overstep
    pub fn from_i16(delta: i16) -> Self {
        Self {
            value: delta.into(),
        }
    }

    /// Returns the number of tick difference (positive or negative) that this TickDelta represents,
    /// rounding to the closes integer value
    pub fn to_i16(&self) -> i16 {
        self.value.to_num()
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

        let mut ticks_f32 = duration.as_secs_f32() / tick_duration.as_secs_f32();
        if is_negative {
            ticks_f32 = -ticks_f32;
        }
        Self {
            value: ticks_f32.wrapping_to_fixed(),
        }
    }

    pub fn zero() -> Self {
        Self {
            value: I16F16::default(),
        }
    }
}

impl Neg for TickDelta {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self { value: -self.value }
    }
}

impl Add for TickDelta {
    type Output = TickDelta;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            value: self.value.wrapping_add(rhs.value),
        }
    }
}

impl Sub for TickDelta {
    type Output = TickDelta;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            value: self.value.wrapping_sub(rhs.value),
        }
    }
}

impl Mul<f32> for TickDelta {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self {
            value: self.value.to_num::<f32>().mul(rhs).wrapping_to_fixed(),
        }
    }
}

impl Mul<U0F16> for TickDelta {
    type Output = Self;

    fn mul(self, rhs: U0F16) -> Self::Output {
        let rhs_fixed: I16F16 = rhs.to_fixed();
        Self {
            value: self.value.wrapping_mul(rhs_fixed),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[reflect(opaque)]
pub struct PositiveTickDelta {
    pub value: U16F16,
}

impl PositiveTickDelta {
    pub const fn lit(src: &str) -> Self {
        Self {
            value: U16F16::lit(src),
        }
    }
    pub fn tick_diff(&self) -> u16 {
        self.value.to_num::<u16>()
    }
    pub fn overstep(&self) -> Overstep {
        Overstep::new(self.value.wrapping_to_num())
    }
}

impl From<TickDelta> for PositiveTickDelta {
    fn from(value: TickDelta) -> Self {
        if value.is_negative() {
            panic!("Cannot convert negative TickDelta to PositiveTickDelta");
        }
        Self {
            value: value.value.cast_unsigned(),
        }
    }
}

impl ToBytes for PositiveTickDelta {
    fn bytes_len(&self) -> usize {
        self.tick_diff().bytes_len() + self.overstep().bytes_len()
    }

    // TODO: use varint for the tick_diff since it's probably small
    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.tick_diff().to_bytes(buffer)?;
        self.overstep().to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let tick_diff = u16::from_bytes(buffer)?;
        let overstep = Overstep::from_bytes(buffer)?;
        Ok(Self {
            value: U16F16::from(tick_diff) + U16F16::from(overstep.value),
        })
    }
}

/// Delta between two instants
///
/// This is mostly useful because it can represent a positive or a negative duration.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct TimeDelta {
    duration: chrono::TimeDelta,
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
            duration: chrono::TimeDelta::from_std(duration)?,
        })
    }
}

impl Neg for TimeDelta {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            duration: -self.duration,
        }
    }
}

impl Add<TickDelta> for TickInstant {
    type Output = TickInstant;

    fn add(self, rhs: TickDelta) -> Self::Output {
        TickInstant {
            value: self.value.wrapping_add_signed(rhs.value),
        }
    }
}

impl Sub<TickDelta> for TickInstant {
    type Output = TickInstant;

    fn sub(self, rhs: TickDelta) -> Self::Output {
        TickInstant {
            value: self.value.wrapping_sub_signed(rhs.value),
        }
    }
}

impl Sub for TickInstant {
    type Output = TickDelta;

    fn sub(self, rhs: TickInstant) -> Self::Output {
        TickDelta {
            value: self.value.cast_signed().wrapping_sub_unsigned(rhs.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::{AbsDiffEq, assert_abs_diff_eq, assert_relative_eq};
    use core::time::Duration;

    impl AbsDiffEq for Overstep {
        type Epsilon = Overstep;

        fn default_epsilon() -> Self::Epsilon {
            Overstep {
                value: U0F16::DELTA,
            }
        }

        fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
            self.value.abs_diff(other.value) <= epsilon.value
        }
    }

    impl AbsDiffEq for TickInstant {
        type Epsilon = TickInstant;

        fn default_epsilon() -> Self::Epsilon {
            TickInstant {
                value: U16F16::DELTA,
            }
        }

        fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
            self.value.abs_diff(other.value) <= epsilon.value
        }
    }

    impl AbsDiffEq for TickDelta {
        type Epsilon = TickDelta;

        fn default_epsilon() -> Self::Epsilon {
            TickDelta {
                value: I16F16::DELTA,
            }
        }

        fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
            self.value.abs_diff(other.value) <= epsilon.value
        }
    }

    #[test]
    fn test_overstep_quantization_error() {
        // Test that the round trip error is less than 1% for values from 0.0 to 1.0
        for i in 0..=10 {
            let original_value = i as f32 / 10.0;
            let overstep = Overstep::from_f32(original_value);
            let quantized = overstep.to_u8();
            let round_trip = Overstep::from_u8(quantized).to_f32();

            assert_relative_eq!(round_trip, original_value, epsilon = 0.01);
        }
    }

    #[test]
    fn test_tickinstant_ordering() {
        let t1 = TickInstant::lit("10.5");
        let t2 = TickInstant::lit("10.7");
        let t3 = TickInstant::lit("11.2");

        assert!(t1 < t2);
        assert!(t2 < t3);
        assert!(t1 < t3);

        assert_eq!(t1.cmp(&t1), Ordering::Equal);
        assert_eq!(t1.cmp(&t2), Ordering::Less);
        assert_eq!(t2.cmp(&t1), Ordering::Greater);
    }

    #[test]
    fn test_tickinstant_add_positive_tickdelta() {
        let tick_instant = TickInstant::lit("10.3");
        let tick_delta = TickDelta::lit("5.2");

        let result = tick_instant + tick_delta;

        assert_abs_diff_eq!(result, TickInstant::lit("15.5"));
    }

    #[test]
    fn test_tickinstant_add_negative_tickdelta() {
        let tick_instant = TickInstant::lit("10.3");
        let tick_delta = TickDelta::lit("-5.2"); // negative delta

        let result = tick_instant + tick_delta;

        assert_abs_diff_eq!(result, TickInstant::lit("5.1"));
    }

    #[test]
    fn test_tickinstant_add_with_overstep_overflow() {
        let tick_instant = TickInstant::lit("10.7");
        let tick_delta = TickDelta::lit("5.6");

        let result = tick_instant + tick_delta;

        // 0.7 + 0.6 = 1.3, which is 1 tick + 0.3 overstep
        assert_abs_diff_eq!(result, TickInstant::lit("16.3"));
    }

    #[test]
    fn test_tickinstant_sub_positive_tickdelta() {
        let tick_instant = TickInstant::lit("10.7");
        let tick_delta = TickDelta::lit("5.2");

        let result = tick_instant - tick_delta;

        assert_abs_diff_eq!(result, TickInstant::lit("5.5"));
    }

    #[test]
    fn test_tickinstant_sub_negative_tickdelta() {
        let tick_instant = TickInstant::lit("10.3");
        let tick_delta = TickDelta::lit("-5.2"); // negative delta

        let result = tick_instant - tick_delta;

        assert_abs_diff_eq!(result, TickInstant::lit("15.5"));
    }

    #[test]
    fn test_tickinstant_sub_with_overstep_underflow() {
        let tick_instant = TickInstant::lit("10.3");
        let tick_delta = TickDelta::lit("5.7");

        let result = tick_instant - tick_delta;

        // 0.3 - 0.7 = -0.4, which becomes 0.6 with borrowing from tick
        assert_abs_diff_eq!(result, TickInstant::lit("4.6"));
    }

    #[test]
    fn test_tickinstant_sub_tickinstant() {
        let t1 = TickInstant::lit("15.7");
        let t2 = TickInstant::lit("10.3");

        // t1 - t2 (positive result)
        let delta = t1 - t2;
        assert_abs_diff_eq!(delta, TickDelta::lit("5.4"));

        // t2 - t1 (negative result)
        let delta = t2 - t1;
        assert_abs_diff_eq!(delta, TickDelta::lit("-5.4"));
    }

    #[test]
    fn test_tickinstant_sub_tickinstant_with_overstep_underflow() {
        let t1 = TickInstant::lit("15.2");
        let t2 = TickInstant::lit("10.7");

        // Need to borrow from tick
        let delta = t1 - t2;
        assert_abs_diff_eq!(delta, TickDelta::lit("4.5"));
    }

    #[test]
    fn test_tickdelta_accessors() {
        let delta = TickDelta::lit("-32768");
        assert_eq!(delta.is_positive(), false);
        assert_eq!(delta.is_negative(), true);
        assert_eq!(delta.tick_diff(), 32768);
        assert_eq!(delta.overstep().to_f32(), 0.0);

        let delta = TickDelta::lit("-32767.75");
        assert_eq!(delta.is_positive(), false);
        assert_eq!(delta.is_negative(), true);
        assert_eq!(delta.tick_diff(), 32767);
        assert_eq!(delta.overstep().to_f32(), 0.75);

        let delta = TickDelta::lit("32767.75");
        assert_eq!(delta.is_positive(), true);
        assert_eq!(delta.is_negative(), false);
        assert_eq!(delta.tick_diff(), 32767);
        assert_eq!(delta.overstep().to_f32(), 0.75);
    }

    #[test]
    fn test_tickdelta_negation() {
        let delta = TickDelta::lit("5.3");
        let negated = -delta;

        assert_abs_diff_eq!(negated, TickDelta::lit("-5.3"));

        // Double negation should return to original
        let double_negated = -negated;

        assert_abs_diff_eq!(double_negated, TickDelta::lit("5.3"));
    }

    #[test]
    fn test_tickdelta_signed_addition() {
        let delta = TickDelta::from_i16(10);

        assert_eq!((delta + delta).to_i16(), 20);
        assert_eq!((delta + (-delta)).to_i16(), 0);
        assert_eq!(((-delta) + delta).to_i16(), 0);
        assert_eq!(((-delta) + (-delta)).to_i16(), -20);
    }

    #[test]
    fn test_tickdelta_multiplication() {
        let delta = TickDelta::lit("10.5");

        // Simple multiplication
        let result = delta * 2.0;
        assert_eq!(result, TickDelta::lit("21"));
        assert_relative_eq!(result.overstep().to_f32(), 0.0);

        // Fractional multiplication
        let result = delta * 1.5;
        assert_eq!(result, TickDelta::lit("15.75"));

        // Multiplication causing overstep overflow
        let delta = TickDelta::lit("10.8");
        let result = delta * 1.5;
        assert_abs_diff_eq!(result, TickDelta::lit("16.2"));
    }

    #[test]
    fn test_tickdelta_subtraction() {
        let delta = TickDelta::from(10i16);
        let sub = delta - TickDelta::from(20i16);
        assert_relative_eq!(sub.to_f32(), -10.0);

        let a = TickDelta::lit("0.1");
        let b = TickDelta::lit("0.6");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), -0.5);

        // Same tick, a > b
        let a = TickDelta::lit("0.8");
        let b = TickDelta::lit("0.3");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), 0.5);

        // Different tick, no underflow
        let a = TickDelta::lit("2.7");
        let b = TickDelta::lit("1.2");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), 1.5);

        // Different tick, underflow
        let a = TickDelta::lit("2.1");
        let b = TickDelta::lit("1.6");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), 0.5);

        // rhs > self, no underflow
        let a = TickDelta::lit("1.2");
        let b = TickDelta::lit("2.7");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), -1.5);

        // rhs > self, underflow
        let a = TickDelta::lit("1.6");
        let b = TickDelta::lit("2.1");
        let sub = a - b;
        assert_relative_eq!(sub.to_f32(), -0.5);
    }

    #[test]
    fn test_tick_conversion_roundtrip() {
        let tick_duration = Duration::from_millis(100);
        let original = TickInstant::lit("15.4");

        // Convert to duration and back
        let duration = original.as_duration(tick_duration);
        let roundtrip = TickInstant::from_duration(duration, tick_duration);

        // Allow for small floating point error in overstep
        assert_eq!(roundtrip.tick(), original.tick());

        assert!((roundtrip.overstep().to_f32() - original.overstep().to_f32()).abs() < 0.01);
    }

    #[test]
    fn test_tickdelta_conversion_roundtrip() {
        let tick_duration = Duration::from_millis(100);

        // Test positive delta
        let original_delta = TickDelta::lit("5.3");
        let time_delta = original_delta.to_time_delta(tick_duration);
        let roundtrip = TickDelta::from_time_delta(time_delta, tick_duration);

        assert_eq!(roundtrip.tick_diff(), original_delta.tick_diff());
        assert!((roundtrip.overstep().to_f32() - original_delta.overstep().to_f32()).abs() < 0.01);
        assert_eq!(roundtrip.is_negative(), original_delta.is_negative());

        // Test negative delta
        let original_delta = TickDelta::lit("-7.6");
        let time_delta = original_delta.to_time_delta(tick_duration);
        let roundtrip = TickDelta::from_time_delta(time_delta, tick_duration);

        assert_eq!(roundtrip.tick_diff(), original_delta.tick_diff());
        assert!((roundtrip.overstep().to_f32() - original_delta.overstep().to_f32()).abs() < 0.01);
        assert_eq!(roundtrip.is_negative(), original_delta.is_negative());
    }
}
