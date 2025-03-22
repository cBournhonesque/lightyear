//! u16 that wraps around when it reaches the maximum value
pub trait WrappedId {
    // return self % total
    // used for sequence buffers
    fn rem(&self, total: usize) -> usize;
}

// macro_rules! wrapping_id {
//     ($struct_name:ident) => {
//         wrapping_id_impl!($struct_name, u16, i16, i32);
//     }; // ($struct_name:ident, u32) => {
//        //     wrapping_id_impl!($struct_name, u32, i32, i64);
//        // };
// }
//
/// Index that wraps around 65536
// TODO: we don't want serialize this with gamma!
macro_rules! wrapping_id {
    ($struct_name:ident) => {
        use crate::_internal::paste;
        paste! {
        mod [<$struct_name:lower _module>] {
            use serde::{Deserialize, Serialize};
            use core::ops::{Add, AddAssign, Deref, Sub};
            use core::cmp::Ordering;
            use bevy::reflect::Reflect;
            use crate::serialize::{SerializationError, reader::{Reader, ReadInteger}, writer::WriteInteger, ToBytes};
            use crate::utils::wrapping_id::{wrapping_diff, WrappedId};

            // define the struct
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

            /// Derive deref so that we don't have to write packet_id.0 in most cases
            impl Deref for $struct_name {
                type Target = u16;
                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl Ord for $struct_name {
                fn cmp(&self, other: &Self) -> Ordering {
                    match wrapping_diff(self.0, other.0) {
                        0 => Ordering::Equal,
                        x if x > 0 => Ordering::Less,
                        x if x < 0 => Ordering::Greater,
                        _ => unreachable!(),
                    }
                }
            }

            impl PartialOrd for $struct_name {
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    Some(self.cmp(other))
                }
            }

            impl Sub for $struct_name {
                type Output = i16;

                fn sub(self, rhs: Self) -> Self::Output {
                    wrapping_diff(rhs.0, self.0)
                }
            }

            impl Sub<u16> for $struct_name {
                type Output = Self;

                fn sub(self, rhs: u16) -> Self::Output {
                    // NOTE: this is only valid for small diffs, it doesn't
                    // handle wrapping correctly with `wrapping_diff`
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
            // impl Add<u16> for $struct_name {
            //     type Output = Self;
            //
            //     fn add(self, rhs: u16) -> Self::Output {
            //         Self(self.0.wrapping_add(rhs))
            //     }
            // }

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

pub(crate) use wrapping_id;

/// Retrieves the wrapping difference of b-a.
/// Wraps around 32768
///
/// # Examples
///
/// ```
/// use lightyear::utils::wrapping_id::wrapping_diff;
/// assert_eq!(wrapping_diff(1, 2), 1);
/// assert_eq!(wrapping_diff(2, 1), -1);
/// assert_eq!(wrapping_diff(65535, 0), 1);
/// assert_eq!(wrapping_diff(0, 65535), -1);
/// assert_eq!(wrapping_diff(0, 32767), 32767);
/// assert_eq!(wrapping_diff(0, 32768), -32768);
/// ```
pub fn wrapping_diff(a: u16, b: u16) -> i16 {
    b.wrapping_sub(a) as i16
}

#[cfg(test)]
mod sequence_compare_tests {
    use super::*;

    wrapping_id!(Id);

    #[test]
    fn test_ordering() {
        assert!(Id(2) > Id(1));
        assert!(Id(1) < Id(2));
        assert!(Id(2) == Id(2));
        assert!(Id(0) > Id(65535));
        assert!(Id(0) < Id(32767));
        assert!(Id(0) > Id(32768));
    }
}

#[cfg(test)]
mod wrapping_diff_tests {
    use super::wrapping_diff;

    #[test]
    fn simple() {
        let a: u16 = 10;
        let b: u16 = 12;

        let result = wrapping_diff(a, b);

        assert_eq!(result, 2);
    }

    #[test]
    fn simple_backwards() {
        let a: u16 = 10;
        let b: u16 = 12;

        let result = wrapping_diff(b, a);

        assert_eq!(result, -2);
    }

    #[test]
    fn max_wrap() {
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(2);

        let result = wrapping_diff(a, b);

        assert_eq!(result, 2);
    }

    #[test]
    fn min_wrap() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);

        let result = wrapping_diff(a, b);

        assert_eq!(result, -2);
    }

    #[test]
    fn max_wrap_backwards() {
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(2);

        let result = wrapping_diff(b, a);

        assert_eq!(result, -2);
    }

    #[test]
    fn min_wrap_backwards() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);

        let result = wrapping_diff(b, a);

        assert_eq!(result, 2);
    }

    #[test]
    fn medium_min_wrap() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(diff);

        let result = i32::from(wrapping_diff(a, b));

        assert_eq!(result, -i32::from(diff));
    }

    #[test]
    fn medium_min_wrap_backwards() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(diff);

        let result = i32::from(wrapping_diff(b, a));

        assert_eq!(result, i32::from(diff));
    }

    #[test]
    fn medium_max_wrap() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(diff);

        let result = i32::from(wrapping_diff(a, b));

        assert_eq!(result, i32::from(diff));
    }

    #[test]
    fn medium_max_wrap_backwards() {
        let diff: u16 = u16::MAX / 2;
        let a: u16 = u16::MAX;
        let b: u16 = a.wrapping_add(diff);

        let result = i32::from(wrapping_diff(b, a));

        assert_eq!(result, -i32::from(diff));
    }
}
