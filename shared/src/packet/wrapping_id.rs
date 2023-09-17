use std::cmp::Ordering;
use std::ops::{Add, AddAssign, Deref, Sub};

/// Index that wraps around 65536
macro_rules! wrapping_id {
    ($struct_name:ident) => {
        // define the struct
        #[derive(bitcode::Encode, bitcode::Decode, Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $struct_name(pub u16);

        /// Derive deref so that we don't have to write packet_id.0 in most cases
        impl Deref for $struct_name {
            type Target = (u16);
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
    };
}

/// Internal id that we assign to each packet sent over the network
wrapping_id!(PacketId);
/// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);

/// Retrieves the wrapping difference of b-a.
/// Wraps around 32768
///
/// # Examples
///
/// ```
/// use lightyear_shared::packet::wrapping_id::wrapping_diff;
/// assert_eq!(wrapping_diff(1, 2), 1);
/// assert_eq!(wrapping_diff(2, 1), -1);
/// assert_eq!(wrapping_diff(65535, 0), 1);
/// assert_eq!(wrapping_diff(0, 65535), -1);
/// assert_eq!(wrapping_diff(0, 32767), 32767);
/// assert_eq!(wrapping_diff(0, 32768), -32768);
/// ```
pub fn wrapping_diff(a: u16, b: u16) -> i16 {
    const MAX: i32 = i16::MAX as i32;
    const MIN: i32 = i16::MIN as i32;
    const ADJUST: i32 = (u16::MAX as i32) + 1;

    let a: i32 = i32::from(a);
    let b: i32 = i32::from(b);

    let mut result = b - a;
    if (MIN..=MAX).contains(&result) {
        result as i16
    } else if b > a {
        result = b - (a + ADJUST);
        if (MIN..=MAX).contains(&result) {
            result as i16
        } else {
            panic!("integer overflow, this shouldn't happen")
        }
    } else {
        result = (b + ADJUST) - a;
        if (MIN..=MAX).contains(&result) {
            result as i16
        } else {
            panic!("integer overflow, this shouldn't happen")
        }
    }
}

#[cfg(test)]
mod sequence_compare_tests {
    use super::PacketId;

    #[test]
    fn test_ordering() {
        assert!(PacketId(2) > PacketId(1));
        assert!(PacketId(1) < PacketId(2));
        assert!(PacketId(2) == PacketId(2));
        assert!(PacketId(0) > PacketId(65535));
        assert!(PacketId(0) < PacketId(32767));
        assert!(PacketId(0) > PacketId(32768));
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
