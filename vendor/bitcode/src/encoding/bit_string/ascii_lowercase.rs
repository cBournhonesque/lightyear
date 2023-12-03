use crate::encoding::bit_string::bit_utils::{pack_lsb, unpack_lsb};
use crate::encoding::bit_string::ByteEncoding;
use crate::encoding::prelude::*;

#[derive(Copy, Clone)]
pub struct AsciiLowercase;

impl AsciiLowercase {
    const DATA_MASK: Word = Word::from_le_bytes([0b00011111; 8]);
    const SET_MASK: Word = Word::from_le_bytes([0b01100000; 8]);
}

impl ByteEncoding for AsciiLowercase {
    const BITS_PER_BYTE: usize = 5;

    #[inline(always)]
    fn validate(word: Word, bytes: usize) -> bool {
        let extra_bits = WORD_BITS - (bytes * u8::BITS as usize);
        word & !Self::DATA_MASK == ((Self::SET_MASK << extra_bits) >> extra_bits)
    }

    #[inline(always)]
    fn pack(word: Word) -> Word {
        pack_lsb::<{ Self::BITS_PER_BYTE }>(word)
    }

    #[inline(always)]
    fn unpack(word: Word) -> Word {
        unpack_lsb::<{ Self::BITS_PER_BYTE }>(word) | Self::SET_MASK
    }
}
