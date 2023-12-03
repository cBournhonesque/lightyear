use super::prelude::*;
use crate::nightly::ilog2_non_zero_u64;
use std::num::NonZeroU64;

#[derive(Copy, Clone)]
pub struct Gamma;
impl Encoding for Gamma {
    fn zigzag(self) -> bool {
        true
    }

    #[inline(always)]
    fn write_u64<const BITS: usize>(self, writer: &mut impl Write, word: Word) {
        debug_assert!(BITS <= WORD_BITS);
        if BITS != WORD_BITS {
            debug_assert_eq!(word, word & ((1 << BITS) - 1));
        }

        // https://en.wikipedia.org/wiki/Elias_gamma_coding
        // Gamma can't encode 0 so add 1.
        if let Some(nz) = NonZeroU64::new(word.wrapping_add(1)) {
            let zero_bits = ilog2_non_zero_u64(nz) as usize;
            writer.write_zeros(zero_bits);

            // Special case max value as BITS zeros.
            if BITS != 64 && word == (u64::MAX >> (64 - BITS)) {
                return;
            }

            let integer_bits = zero_bits + 1;

            // Rotate bits mod `integer_bits` instead of reversing since it's faster.
            // 00001bbb -> 0000bbb1
            let rotated = (nz.get() << 1 & !((1 << 1) << zero_bits)) | 1;
            writer.write_bits(rotated, integer_bits);
        } else {
            // Special case u64::MAX as as 64 zeros (based on u64::MAX + 1 == 0 so we skip branch in ilog2).
            writer.write_zeros(64);
        }
    }

    #[inline(always)]
    fn read_u64<const BITS: usize>(self, reader: &mut impl Read) -> Result<Word> {
        debug_assert!((1..=WORD_BITS).contains(&BITS));

        let peek = reader.peek_bits()?;
        let zero_bits = peek.trailing_zeros() as usize;

        let fast = zero_bits < BITS.min(u32::BITS as usize);
        if fast {
            let integer_bits = zero_bits + 1;
            let gamma_bits = zero_bits + integer_bits;
            reader.advance(gamma_bits);

            let rotated = peek >> zero_bits & ((1 << integer_bits) - 1);

            // Rotate bits mod `integer_bits` instead of reversing since it's faster.
            // 0000bbb1 -> 00001bbb
            let v = (rotated >> 1) | (1 << (integer_bits - 1));

            // Gamma can't encode 0 so sub 1.
            let v = v - 1;
            Ok(v)
        } else {
            // The representation is > 64 bits or it's the max value.
            #[cold]
            fn slow<const BITS: usize>(reader: &mut impl Read) -> Result<Word> {
                // True if the representation can't be > 64 bits so it's the max value.
                let always_special_case = BITS < u32::BITS as usize;
                if always_special_case {
                    reader.advance(BITS);
                    return Ok(u64::MAX >> (64 - BITS));
                }

                let zero_bits = (reader.peek_bits()?.trailing_zeros() as usize).min(BITS);
                reader.advance(zero_bits);

                // Max value is special cased as as BITS zeros.
                if zero_bits == BITS {
                    return Ok(u64::MAX >> (64 - BITS));
                }

                let integer_bits = zero_bits + 1;
                let rotated = reader.read_bits(integer_bits)?;

                let v = (rotated >> 1) | (1 << (integer_bits - 1));
                Ok(v - 1)
            }
            slow::<BITS>(reader)
        }
    }
}

#[cfg(all(test, not(miri)))]
mod benches {
    mod u8 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::Gamma, dataset::<u8>);
    }

    mod u16 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::Gamma, dataset::<u16>);
    }

    mod u32 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::Gamma, dataset::<u32>);
    }

    mod u64 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::Gamma, dataset::<u64>);
    }
}

#[cfg(all(test, debug_assertions, not(miri)))]
mod tests {
    use super::*;
    use crate::encoding::prelude::test_prelude::*;

    #[test]
    fn test() {
        fn t<V: Encode + Decode + Copy + Debug + PartialEq>(value: V) {
            test_encoding(Gamma, value)
        }

        for i in 0..u8::MAX {
            t(i);
        }

        t(u16::MAX);
        t(u32::MAX);
        t(u64::MAX);

        t(-1i8);
        t(-1i16);
        t(-1i32);
        t(-1i64);

        #[derive(Debug, PartialEq, Encode, Decode)]
        struct GammaInt<T>(#[bitcode_hint(gamma)] T);

        for i in -7..=7i64 {
            // Zig-zag means that low magnitude signed ints are under one byte.
            assert_eq!(bitcode::encode(&GammaInt(i)).unwrap().len(), 1);
        }
    }
}
