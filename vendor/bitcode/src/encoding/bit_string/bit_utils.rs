use crate::word::Word;
use bitintr::{Pdep, Pext};

#[inline(always)]
pub fn pack_lsb<const BITS: usize>(word: Word) -> Word {
    let mask = Word::from_le_bytes([(1 << BITS) - 1; 8]);

    if cfg!(all(target_feature = "bmi2", not(miri))) {
        word.pext(mask)
    } else {
        // Mask off bits that we don't care about.
        let bytes = (word & mask).to_le_bytes();
        let mut ret1 = 0;
        for (i, &b) in bytes[..4].iter().enumerate() {
            ret1 |= (b as u32) << (i * BITS);
        }
        let mut ret2 = 0;
        for (i, &b) in bytes[4..].iter().enumerate() {
            ret2 |= (b as u32) << (i * BITS);
        }

        // 2 steps + merge is a tiny bit faster.
        ret1 as u64 | (ret2 as u64) << (BITS * 4)
    }
}

#[inline(always)]
pub fn unpack_lsb<const BITS: usize>(word: Word) -> Word {
    if cfg!(all(target_feature = "bmi2", not(miri))) {
        let mask = Word::from_le_bytes([(1 << BITS) - 1; 8]);
        word.pdep(mask)
    } else {
        let mut bytes = [0u8; 8];

        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (word >> (i * BITS) & ((1 << BITS) - 1)) as u8;
        }

        Word::from_le_bytes(bytes)
    }
}
