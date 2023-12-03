use crate::encoding::prelude::*;

#[derive(Copy, Clone)]
pub struct ExpectedRangeU64<const MIN: u64, const MAX: u64>;

impl<const MIN: u64, const MAX: u64> ExpectedRangeU64<MIN, MAX> {
    const RANGE: u64 = MAX - MIN;
    const _A: () = assert!(Self::RANGE < u64::MAX / 2);

    const fn range_bits(self) -> usize {
        ilog2_u64(Self::RANGE.next_power_of_two()) as usize
    }

    const fn invalid_bit_pattern(self) -> Option<u64> {
        if Self::RANGE.is_power_of_two() {
            None
        } else {
            Some(Self::RANGE)
        }
    }

    const fn has_header_bit(self) -> bool {
        self.invalid_bit_pattern().is_none()
    }

    const fn total_bits(self) -> usize {
        self.range_bits() + self.has_header_bit() as usize
    }

    const fn is_pointless(self, bits: usize) -> bool {
        bits <= self.total_bits()
    }
}

impl<const MIN: u64, const MAX: u64> Encoding for ExpectedRangeU64<MIN, MAX> {
    #[inline(always)]
    fn write_u64<const BITS: usize>(self, writer: &mut impl Write, word: Word) {
        // Don't use use this encoding if it's pointless.
        if self.is_pointless(BITS) {
            writer.write_bits(word, BITS);
            return;
        }

        // TODO could extend min and max.
        if (MIN..MAX).contains(&word) {
            let value = word - MIN;
            let header_bit = self.has_header_bit() as u64;
            let value_with_header = (value << header_bit) | header_bit;
            writer.write_bits(value_with_header, self.total_bits());
        } else {
            #[cold]
            fn cold<const MIN: u64, const MAX: u64>(
                me: ExpectedRangeU64<MIN, MAX>,
                word: Word,
                bits: usize,
                writer: &mut impl Write,
            ) {
                if let Some(invalid_bit_pattern) = me.invalid_bit_pattern() {
                    writer.write_bits(invalid_bit_pattern, me.range_bits());
                    writer.write_bits(word, bits);
                } else {
                    writer.write_false();
                    writer.write_bits(word, bits);
                }
            }
            cold(self, word, BITS, writer);
        }
    }

    #[inline(always)]
    fn read_u64<const BITS: usize>(self, reader: &mut impl Read) -> Result<Word> {
        // Don't use use this encoding if it's pointless.
        if self.is_pointless(BITS) {
            return reader.read_bits(BITS);
        }

        let raw_bits = reader.peek_bits()?;
        let total_bits = self.total_bits();

        let value_and_header = raw_bits & ((1 << total_bits) - 1);
        if let Some(invalid_bit_pattern) = self.invalid_bit_pattern() {
            if value_and_header != invalid_bit_pattern {
                reader.advance(total_bits);

                let value = value_and_header;
                let word = value + MIN;
                if BITS < WORD_BITS && word >= (1 << BITS) {
                    Err(E::Invalid("expected range").e())
                } else {
                    Ok(word)
                }
            } else {
                #[cold]
                fn cold(reader: &mut impl Read, bits: usize, skip: usize) -> Result<Word> {
                    reader.advance(skip);
                    reader.read_bits(bits)
                }
                cold(reader, BITS, self.range_bits())
            }
        } else if value_and_header & 1 != 0 {
            reader.advance(total_bits);

            let value = value_and_header >> 1;
            let word = value + MIN;
            if BITS < WORD_BITS && word >= (1 << BITS) {
                Err(E::Invalid("expected range").e())
            } else {
                Ok(word)
            }
        } else {
            #[cold]
            fn cold(reader: &mut impl Read, bits: usize) -> Result<Word> {
                reader.advance(1);
                reader.read_bits(bits)
            }
            cold(reader, BITS)
        }
    }
}

#[cfg(all(test, not(miri)))]
mod benches {
    use crate::encoding::prelude::bench_prelude::*;
    use rand::prelude::*;

    fn dataset() -> Vec<u64> {
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(Default::default());
        (0..1000).map(|_| rng.gen_range(0..100)).collect()
    }

    bench_encoding!(super::ExpectedRangeU64::<0, 100>, dataset);
}

#[cfg(all(test, debug_assertions, not(miri)))]
mod tests {
    use super::*;
    use crate::encoding::prelude::test_prelude::*;

    #[test]
    fn test() {
        fn t<V: Encode + Decode + Copy + Debug + PartialEq>(value: V) {
            let encoding: ExpectedRangeU64<0, 10> = ExpectedRangeU64;
            test_encoding(encoding, value);

            let encoding: ExpectedRangeU64<0, 16> = ExpectedRangeU64;
            test_encoding(encoding, value);
        }

        for i in 0..u8::MAX {
            t(i);
        }

        t(u16::MAX);
        t(u32::MAX);
        t(u64::MAX);

        #[derive(Copy, Clone, Debug, PartialEq, Encode, Decode)]
        struct IntLessThan1<T>(#[bitcode_hint(expected_range = "0..1")] T);

        for i in 0..1u8 {
            let bits_required = bitcode::encode(&[IntLessThan1(i); 8]).unwrap().len();
            // 1 bits are required.
            assert_eq!(bits_required, 1);
        }

        for i in 1..10u8 {
            let bits_required = bitcode::encode(&[IntLessThan1(i); 8]).unwrap().len();
            assert_eq!(bits_required, 9);
        }

        #[derive(Copy, Clone, Debug, PartialEq, Encode, Decode)]
        struct IntLessThan10<T>(#[bitcode_hint(expected_range = "0..10")] T);

        for i in 0..10u8 {
            let bits_required = bitcode::encode(&[IntLessThan10(i); 8]).unwrap().len();
            // Only 4 bits are required since there are invalid bit patterns to use.
            assert_eq!(bits_required, 4);
        }

        for i in 10..20u8 {
            let bits_required = bitcode::encode(&[IntLessThan10(i); 8]).unwrap().len();
            assert_eq!(bits_required, 12);
        }

        #[derive(Copy, Clone, Debug, PartialEq, Encode, Decode)]
        struct IntLessThan16<T>(#[bitcode_hint(expected_range = "0..16")] T);

        for i in 0..16u8 {
            let bits_required = bitcode::encode(&[IntLessThan16(i); 8]).unwrap().len();
            // 5 bits are required since there aren't invalid bit patterns to use.
            assert_eq!(bits_required, 5);
        }

        for i in 16..32u8 {
            let bits_required = bitcode::encode(&[IntLessThan16(i); 8]).unwrap().len();
            assert_eq!(bits_required, 9);
        }
    }
}
