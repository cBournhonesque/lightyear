use crate::encoding::bit_string::bit_utils::{pack_lsb, unpack_lsb};
use crate::encoding::bit_string::ByteEncoding;
use crate::encoding::prelude::*;

#[derive(Copy, Clone)]
pub struct Ascii;

impl Ascii {
    const MASK: Word = Word::from_le_bytes([0x7F; 8]);
}

impl ByteEncoding for Ascii {
    const BITS_PER_BYTE: usize = 7;

    #[inline(always)]
    fn validate(word: Word, _: usize) -> bool {
        word & !Self::MASK == 0
    }

    #[inline(always)]
    fn pack(word: Word) -> Word {
        pack_lsb::<{ Self::BITS_PER_BYTE }>(word)
    }

    #[inline(always)]
    fn unpack(word: Word) -> Word {
        unpack_lsb::<{ Self::BITS_PER_BYTE }>(word)
    }
}
