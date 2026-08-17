//! `u16` identifier with wrapping sequence-number arithmetic.
pub trait WrappedId {
    /// returns self % total
    fn rem(&self, total: usize) -> usize;
}

pub use pastey::paste;

/// Index that wraps around 2^16.
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
            use lightyear_utils::wrapping_id::{WrappedId, wrapping_diff};

            /// A wrapping sequence identifier.
            ///
            /// This type intentionally does not implement [`Ord`]: circular sequence order is
            /// meaningful only relative to a nearby anchor and cannot satisfy a global ordering
            /// contract. Use equality-based containers for lookup and keep chronological order in
            /// the owning data structure.
            #[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, Hash, PartialEq, Default, Reflect
            )]
            pub struct $struct_name(pub u16);

            impl ToBytes for $struct_name {
                fn bytes_len(&self) -> usize {
                    2
                }

                fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
                    Ok(buffer.write_u16(self.0)?)
                }

                fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
                where
                    Self: Sized,
                {
                    Ok(Self(buffer.read_u16()?))
                }
            }

            impl WrappedId for $struct_name {
                 fn rem(&self, total: usize) -> usize {
                     (self.0 as usize) % total
                 }
            }

            impl From<u16> for $struct_name {
                fn from(value: u16) -> Self {
                    Self(value)
                }
            }

            impl Deref for $struct_name {
                type Target = u16;
                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl PartialOrd for $struct_name {
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    // Relative comparisons require the caller to keep both sequence numbers
                    // within a window smaller than half the u16 range. Values immediately after
                    // rollover therefore compare newer than values immediately before rollover.
                    Some(match wrapping_diff(self.0, other.0) {
                        0 => Ordering::Equal,
                        x if x > 0 => Ordering::Less,
                        x if x < 0 => Ordering::Greater,
                        _ => unreachable!(),
                    })
                }
            }

            impl Sub for $struct_name {
                type Output = i16;

                fn sub(self, rhs: Self) -> Self::Output {
                    self.0.wrapping_sub(rhs.0) as i16
                }
            }

            impl Sub<u16> for $struct_name {
                type Output = Self;

                fn sub(self, rhs: u16) -> Self::Output {
                    Self(self.0.wrapping_sub(rhs))
                }
            }

            impl Add for $struct_name {
                type Output = Self;

                fn add(self, rhs: Self) -> Self::Output {
                    Self(self.0.wrapping_add(rhs.0))
                }
            }

            impl AddAssign<u16> for $struct_name {
                fn add_assign(&mut self, rhs: u16) {
                    self.0 = self.0.wrapping_add(rhs);
                }
            }

            impl Add<i16> for $struct_name {
                type Output = Self;

                fn add(self, rhs: i16) -> Self::Output {
                    Self(self.0.wrapping_add_signed(rhs))
                }
            }
        }
        pub use [<$struct_name:lower _module>]::$struct_name;
        }
    };
}

/// Retrieves the wrapping difference of b-a.
///
/// The result is meaningful when the two IDs are less than half the `u16`
/// sequence space apart.
///
/// # Examples
///
/// ```
/// use lightyear_utils::wrapping_id::wrapping_diff;
/// assert_eq!(wrapping_diff(1, 2), 1);
/// assert_eq!(wrapping_diff(2, 1), -1);
/// assert_eq!(wrapping_diff(u16::MAX, 0), 1);
/// assert_eq!(wrapping_diff(0, u16::MAX), -1);
/// ```
pub fn wrapping_diff(a: u16, b: u16) -> i16 {
    b.wrapping_sub(a) as i16
}

#[cfg(test)]
mod wrapping_diff_tests {
    use super::wrapping_diff;

    #[test]
    fn simple() {
        let a: u16 = 10;
        let b: u16 = 12;
        assert_eq!(wrapping_diff(a, b), 2);
    }

    #[test]
    fn simple_backwards() {
        let a: u16 = 10;
        let b: u16 = 12;
        assert_eq!(wrapping_diff(b, a), -2);
    }

    #[test]
    fn max_wrap() {
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(2);
        assert_eq!(wrapping_diff(a, b), 2);
    }

    #[test]
    fn min_wrap() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);
        assert_eq!(wrapping_diff(a, b), -2);
    }

    #[test]
    fn max_wrap_backwards() {
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(2);
        assert_eq!(wrapping_diff(b, a), -2);
    }

    #[test]
    fn min_wrap_backwards() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);
        assert_eq!(wrapping_diff(b, a), 2);
    }

    #[test]
    fn medium_min_wrap() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(diff);
        let result = wrapping_diff(a, b) as i64;
        assert_eq!(result, -(diff as i64));
    }

    #[test]
    fn medium_max_wrap() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(diff);
        let result = wrapping_diff(a, b) as i64;
        assert_eq!(result, diff as i64);
    }
}
