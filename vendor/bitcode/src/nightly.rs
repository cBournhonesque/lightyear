// Replacements for nightly features used while developing the crate.

use std::num::NonZeroU64;

#[inline(always)]
pub const fn div_ceil(me: usize, rhs: usize) -> usize {
    let d = me / rhs;
    let r = me % rhs;
    if r > 0 && rhs > 0 {
        d + 1
    } else {
        d
    }
}

#[inline(always)]
pub const fn ilog2_u64(me: u64) -> u32 {
    if cfg!(debug_assertions) && me == 0 {
        panic!("log2 on zero")
    }
    u64::BITS - 1 - me.leading_zeros()
}

// Faster than ilog2_u64 on CPUs that have bsr but not lzcnt.
#[inline(always)]
pub const fn ilog2_non_zero_u64(me: NonZeroU64) -> u32 {
    u64::BITS - 1 - me.leading_zeros()
}

/// `<usize as Ord>::min` isn't const yet.
pub const fn min(a: usize, b: usize) -> usize {
    if a < b {
        a
    } else {
        b
    }
}

/// `<usize as Ord>::max` isn't const yet.
pub const fn max(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}
