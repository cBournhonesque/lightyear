use std::fmt;

/// Error returned when constructing a [`VarInt`] from a value >= 2^62
#[derive(Debug)]
pub struct VarIntBoundsExceeded;

/// QUIC variable-length integer.
///
/// A non-negative integer value, less than 2^62.
#[derive(Default, Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VarInt(u64);

impl VarInt {
    /// The largest value that can be represented by this integer type.
    pub const MAX: Self = Self(4_611_686_018_427_387_903);

    /// The smallest value that can be represented by this integer type.
    pub const MIN: Self = Self(0);

    /// Maximum number of bytes for varint encoding.
    pub const MAX_SIZE: usize = 8;

    /// Constructs a [`VarInt`] from `u32`.
    #[inline(always)]
    pub const fn from_u32(value: u32) -> Self {
        Self(value as u64)
    }

    /// Tries to construct a [`VarInt`] from `u64`.
    #[inline(always)]
    pub const fn try_from_u64(value: u64) -> Result<Self, VarIntBoundsExceeded> {
        if value <= Self::MAX.0 {
            Ok(Self(value))
        } else {
            Err(VarIntBoundsExceeded)
        }
    }

    /// Creates a [`VarInt`] without ensuring it's in range.
    ///
    /// # Safety
    ///
    /// `value` must be less than 2^62.
    #[inline(always)]
    pub const unsafe fn from_u64_unchecked(value: u64) -> Self {
        debug_assert!(value <= Self::MAX.into_inner());
        Self(value)
    }

    /// Extracts the integer value as `u64`.
    #[inline(always)]
    pub const fn into_inner(self) -> u64 {
        self.0
    }

    /// Returns how many bytes it would take to encode this value as
    /// a variable-length integer.
    ///
    /// This value cannot be larger than [`Self::MAX_SIZE`].
    pub const fn size(self) -> usize {
        if self.0 <= 63 {
            1
        } else if self.0 <= 16383 {
            2
        } else if self.0 <= 1_073_741_823 {
            4
        } else if self.0 <= 4_611_686_018_427_387_903 {
            8
        } else {
            unreachable!()
        }
    }

    /// Returns how long the variable-length integer is, given its first byte.
    pub const fn parse_size(first: u8) -> usize {
        match first >> 6 {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            _ => unreachable!(),
        }
    }
}

impl From<u8> for VarInt {
    #[inline(always)]
    fn from(value: u8) -> Self {
        Self::from_u32(u32::from(value))
    }
}

impl From<u16> for VarInt {
    #[inline(always)]
    fn from(value: u16) -> Self {
        Self::from_u32(u32::from(value))
    }
}

impl From<u32> for VarInt {
    #[inline(always)]
    fn from(value: u32) -> Self {
        Self::from_u32(value)
    }
}

impl TryFrom<u64> for VarInt {
    type Error = VarIntBoundsExceeded;

    #[inline(always)]
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_from_u64(value)
    }
}

impl From<VarInt> for u64 {
    #[inline]
    fn from(value: VarInt) -> Self {
        value.0
    }
}

impl fmt::Debug for VarInt {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for VarInt {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        assert!(VarInt::try_from_u64(VarInt::MAX.into_inner()).is_ok());
        assert!(VarInt::try_from_u64(VarInt::MAX.into_inner() + 1).is_err());
        assert!(VarInt::try_from_u64(2_u64.pow(62)).is_err());
        assert!(VarInt::try_from_u64(2_u64.pow(62) - 1).is_ok());
    }

    #[test]
    fn size() {
        assert!((1..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(0).unwrap().size()));
        assert!((1..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(63).unwrap().size()));

        assert!((2..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(64).unwrap().size()));
        assert!((2..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(16383).unwrap().size()));

        assert!((4..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(16384).unwrap().size()));
        assert!(
            (4..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(1_073_741_823).unwrap().size())
        );

        assert!(
            (8..=VarInt::MAX_SIZE).contains(&VarInt::try_from_u64(1_073_741_824).unwrap().size())
        );
        assert!((8..=VarInt::MAX_SIZE).contains(
            &VarInt::try_from_u64(4_611_686_018_427_387_903)
                .unwrap()
                .size()
        ));

        assert_eq!(VarInt::MAX_SIZE, 8);
        assert_eq!(VarInt::MAX.size(), VarInt::MAX_SIZE);
    }

    #[test]
    fn parse() {
        assert_eq!(VarInt::parse_size(0xc2), 8);
        assert_eq!(VarInt::parse_size(0x9d), 4);
        assert_eq!(VarInt::parse_size(0x7b), 2);
        assert_eq!(VarInt::parse_size(0x25), 1);
    }
}
