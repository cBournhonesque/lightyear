use crate::encoding::Encoding;
use crate::read::Read;
use crate::{Decode, Result, E};

pub const ZST_LIMIT: usize = 1 << 16;

fn check_zst_len(len: usize) -> Result<()> {
    if len > ZST_LIMIT {
        Err(E::Invalid("too many zst").e())
    } else {
        Ok(())
    }
}

// Used by deserialize. Guards against Vec<()> with huge len taking forever.
#[inline]
#[cfg(any(test, feature = "serde"))]
pub fn guard_zst<T>(len: usize) -> Result<()> {
    if std::mem::size_of::<T>() == 0 {
        check_zst_len(len)
    } else {
        Ok(())
    }
}

// Used by decode. Guards against allocating huge Vec<T> without enough remaining bits to fill it.
// Also guards against Vec<()> with huge len taking forever.
#[inline]
pub fn guard_len<T: Decode>(len: usize, encoding: impl Encoding, reader: &impl Read) -> Result<()> {
    // In #[derive(Decode)] we report serde types as 1 bit min even though they might serialize
    // to 0. We do this so we can have large vectors past the ZST_LIMIT. We assume that any type
    // that will serialize to nothing in serde has no size.
    if T::DECODE_MIN == 0 || std::mem::size_of::<T>() == 0 {
        check_zst_len(len)
    } else {
        // If we are using an encoding other than fixed DECODE_MIN is invalid.
        let min_bits = if encoding.is_fixed() {
            T::DECODE_MIN
        } else {
            1
        };

        // We ensure that we have the minimum required bits so decoding doesn't allocate unbounded memory.
        let bits = len.saturating_mul(min_bits);
        reader.reserve_bits(bits)
    }
}
