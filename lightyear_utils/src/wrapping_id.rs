//! u32 that wraps around when it reaches the maximum value.
//!
//! In practice with u32, wrapping never occurs during a game session
//! (~828 days at 60 Hz). The wrapping arithmetic is kept for correctness
//! but can be treated as plain integer arithmetic.
pub trait WrappedId {
    /// returns self % total
    fn rem(&self, total: usize) -> usize;
}

pub use paste::paste;

/// Index that wraps around 2^32
#[macro_export]
macro_rules! wrapping_id {
    ($struct_name:ident) => {
        use lightyear_utils::wrapping_id::paste;
        paste! {
        mod [<$struct_name:lower _module>] {
            use serde::{Deserialize, Serialize};
            use core::ops::{Add, AddAssign, Deref, Sub};
            use core::cmp::Ordering;
            use bevy_reflect::Reflect;
            use lightyear_serde::{SerializationError, reader::{Reader, ReadInteger}, writer::WriteInteger, ToBytes};
            use lightyear_utils::wrapping_id::WrappedId;

            #[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, Hash, PartialEq, Default, Reflect
            )]
            pub struct $struct_name(pub u32);

            impl ToBytes for $struct_name {
                fn bytes_len(&self) -> usize {
                    4
                }

                fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
                    Ok(buffer.write_u32(self.0)?)
                }

                fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
                where
                    Self: Sized,
                {
                    Ok(Self(buffer.read_u32()?))
                }
            }

            impl WrappedId for $struct_name {
                 fn rem(&self, total: usize) -> usize {
                     (self.0 as usize) % total
                 }
            }

            impl From<u32> for $struct_name {
                fn from(value: u32) -> Self {
                    Self(value)
                }
            }

            impl Deref for $struct_name {
                type Target = u32;
                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl Ord for $struct_name {
                fn cmp(&self, other: &Self) -> Ordering {
                    self.0.cmp(&other.0)
                }
            }

            impl PartialOrd for $struct_name {
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    Some(self.cmp(other))
                }
            }

            impl Sub for $struct_name {
                type Output = i32;

                fn sub(self, rhs: Self) -> Self::Output {
                    (self.0 as i64 - rhs.0 as i64) as i32
                }
            }

            impl Sub<u32> for $struct_name {
                type Output = Self;

                fn sub(self, rhs: u32) -> Self::Output {
                    Self(self.0.saturating_sub(rhs))
                }
            }

            impl Add for $struct_name {
                type Output = Self;

                fn add(self, rhs: Self) -> Self::Output {
                    Self(self.0.saturating_add(rhs.0))
                }
            }

            impl AddAssign<u32> for $struct_name {
                fn add_assign(&mut self, rhs: u32) {
                    self.0 = self.0.saturating_add(rhs);
                }
            }

            impl Add<i32> for $struct_name {
                type Output = Self;

                fn add(self, rhs: i32) -> Self::Output {
                    Self(self.0.saturating_add_signed(rhs))
                }
            }
        }
        pub use [<$struct_name:lower _module>]::$struct_name;
        }
    };
}

/// Retrieves the wrapping difference of b-a.
///
/// With u32, wrapping only occurs after ~828 days at 60 Hz, so in practice
/// this is equivalent to plain `(b - a) as i32`.
///
/// # Examples
///
/// ```
/// use lightyear_utils::wrapping_id::wrapping_diff;
/// assert_eq!(wrapping_diff(1, 2), 1);
/// assert_eq!(wrapping_diff(2, 1), -1);
/// assert_eq!(wrapping_diff(u32::MAX, 0), 1);
/// assert_eq!(wrapping_diff(0, u32::MAX), -1);
/// ```
pub fn wrapping_diff(a: u32, b: u32) -> i32 {
    b.wrapping_sub(a) as i32
}

#[cfg(test)]
mod wrapping_diff_tests {
    use super::wrapping_diff;

    #[test]
    fn simple() {
        let a: u32 = 10;
        let b: u32 = 12;
        assert_eq!(wrapping_diff(a, b), 2);
    }

    #[test]
    fn simple_backwards() {
        let a: u32 = 10;
        let b: u32 = 12;
        assert_eq!(wrapping_diff(b, a), -2);
    }

    #[test]
    fn max_wrap() {
        let a: u32 = u32::MAX;
        let b: u32 = a.wrapping_add(2);
        assert_eq!(wrapping_diff(a, b), 2);
    }

    #[test]
    fn min_wrap() {
        let a: u32 = 0;
        let b: u32 = a.wrapping_sub(2);
        assert_eq!(wrapping_diff(a, b), -2);
    }

    #[test]
    fn max_wrap_backwards() {
        let a: u32 = u32::MAX;
        let b: u32 = a.wrapping_add(2);
        assert_eq!(wrapping_diff(b, a), -2);
    }

    #[test]
    fn min_wrap_backwards() {
        let a: u32 = 0;
        let b: u32 = a.wrapping_sub(2);
        assert_eq!(wrapping_diff(b, a), 2);
    }

    #[test]
    fn medium_min_wrap() {
        let diff: u32 = u32::MAX / 2;
        let a: u32 = 0;
        let b: u32 = a.wrapping_sub(diff);
        let result = wrapping_diff(a, b) as i64;
        assert_eq!(result, -(diff as i64));
    }

    #[test]
    fn medium_max_wrap() {
        let diff: u32 = u32::MAX / 2;
        let a: u32 = u32::MAX;
        let b: u32 = a.wrapping_add(diff);
        let result = wrapping_diff(a, b) as i64;
        assert_eq!(result, diff as i64);
    }
}
