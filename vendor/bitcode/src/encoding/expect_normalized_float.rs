use crate::code::{Decode, Encode};
use crate::encoding::prelude::*;
use crate::encoding::Fixed;

#[derive(Copy, Clone)]
pub struct ExpectNormalizedFloat;

// Cannot currently be more than 12 because that would make f64 > 64 bits (requiring multiple reads/writes).
const MAX_EXP_ZEROS: usize = 12;

macro_rules! impl_float {
    ($write:ident, $read:ident, $t:ty, $i: ty, $mantissa:literal, $exp_bias: literal) => {
        #[inline(always)]
        fn $write(self, writer: &mut impl Write, v: $t) {
            let mantissa_bits = $mantissa as usize;
            let exp_bias = $exp_bias as u32;
            let sign_bit = 1 << (<$i>::BITS - 1);

            let bits = v.to_bits();
            let sign = bits & sign_bit;
            let bits_without_sign = bits & !sign_bit;
            let exp = (bits_without_sign >> mantissa_bits) as u32;
            let exp_zeros = (exp_bias - 1).wrapping_sub(exp) as usize;

            if (sign | exp_zeros as $i) < MAX_EXP_ZEROS as $i {
                let mantissa = bits as $i & !(<$i>::MAX << mantissa_bits);
                let v = (((mantissa as u64) << 1) | 1) << exp_zeros;
                writer.write_bits(v, mantissa_bits + exp_zeros + 1);
            } else {
                #[cold]
                fn cold(writer: &mut impl Write, v: $t) {
                    writer.write_zeros(MAX_EXP_ZEROS);
                    v.encode(Fixed, writer).unwrap()
                }
                cold(writer, v);
            }
        }

        #[inline(always)]
        fn $read(self, reader: &mut impl Read) -> Result<$t> {
            let mantissa_bits = $mantissa as usize;
            let exp_bias = $exp_bias as u32;

            let v = reader.peek_bits()?;
            let exp_zeros = v.trailing_zeros() as usize;

            if exp_zeros < MAX_EXP_ZEROS {
                let exp_bits = exp_zeros + 1;
                reader.advance(mantissa_bits + exp_bits);

                let mantissa = (v >> exp_bits) as $i & !(<$i>::MAX << mantissa_bits);
                let exp = (exp_bias - 1) - exp_zeros as u32;
                Ok(<$t>::from_bits(exp as $i << mantissa_bits | mantissa))
            } else {
                #[cold]
                fn cold(reader: &mut impl Read) -> Result<$t> {
                    reader.advance(MAX_EXP_ZEROS);
                    <$t>::decode(Fixed, reader)
                }
                cold(reader)
            }
        }
    }
}

impl Encoding for ExpectNormalizedFloat {
    impl_float!(write_f32, read_f32, f32, u32, 23, 127);
    impl_float!(write_f64, read_f64, f64, u64, 52, 1023);
}

#[cfg(all(test, not(miri)))]
mod benches {
    mod f32 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::ExpectNormalizedFloat, dataset::<f32>);
    }

    mod f64 {
        use crate::encoding::bench_prelude::*;
        bench_encoding!(crate::encoding::ExpectNormalizedFloat, dataset::<f64>);
    }
}

#[cfg(all(test, debug_assertions, not(miri)))]
mod tests {
    macro_rules! impl_test {
        ($t:ty, $i:ty) => {
            use crate::encoding::expect_normalized_float::*;
            use crate::encoding::prelude::test_prelude::*;
            use rand::{Rng, SeedableRng};

            fn t(value: $t) {
                #[derive(Copy, Clone, Debug, Encode, Decode)]
                struct ExactBits(#[bitcode_hint(expected_range = "0.0..1.0")] $t);

                impl PartialEq for ExactBits {
                    fn eq(&self, other: &Self) -> bool {
                        self.0.to_bits() == other.0.to_bits()
                    }
                }
                test_encoding(ExpectNormalizedFloat, ExactBits(value));
            }

            #[test]
            fn test_random() {
                let mut rng = rand_chacha::ChaCha20Rng::from_seed(Default::default());
                for _ in 0..100000 {
                    let f = <$t>::from_bits(rng.gen::<$i>());
                    t(f)
                }
            }

            #[test]
            fn test2() {
                t(0.0);
                t(0.5);
                t(1.0);
                t(-1.0);
                t(<$t>::INFINITY);
                t(<$t>::NEG_INFINITY);
                t(<$t>::NAN);
                t(0.0000000000001);

                fn normalized_floats(n: usize) -> impl Iterator<Item = $t> {
                    let scale = 1.0 / n as $t;
                    (0..n).map(move |i| i as $t * scale)
                }

                fn normalized_float_bits(n: usize) -> $t {
                    use crate::buffer::BufferTrait;
                    use crate::word_buffer::WordBuffer;

                    let mut buffer = WordBuffer::default();
                    let mut writer = buffer.start_write();
                    for v in normalized_floats(n) {
                        v.encode(ExpectNormalizedFloat, &mut writer).unwrap();
                    }
                    let bytes = buffer.finish_write(writer).to_vec();

                    let (mut reader, context) = buffer.start_read(&bytes);
                    for v in normalized_floats(n) {
                        let decoded = <$t>::decode(ExpectNormalizedFloat, &mut reader).unwrap();
                        assert_eq!(decoded, v);
                    }
                    WordBuffer::finish_read(reader, context).unwrap();

                    (bytes.len() * u8::BITS as usize) as $t / n as $t
                }

                if <$i>::BITS == 32 {
                    assert!((25.0..25.5).contains(&normalized_float_bits(1 << 12)));
                    // panic!("bits {}", normalized_float_bits(6000000)); // bits 25.013674
                } else {
                    assert!((54.0..54.5).contains(&normalized_float_bits(1 << 12)));
                    // panic!("bits {}", normalized_float_bits(6000000)); // bits 54.019532
                }
            }
        };
    }

    mod f32 {
        impl_test!(f32, u32);
    }

    mod f64 {
        impl_test!(f64, u64);
    }
}
