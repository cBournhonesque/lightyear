use crate::word::Word;

/// Abstracts over writing bits to a buffer.
pub trait Write {
    /// Writes a bit. If `v` is always `false` use [`Self::write_false`].
    fn write_bit(&mut self, v: bool);
    /// Writes up to 64 bits. The index of `word`'s most significant 1 must be < `bits`.
    /// `bits` must be in range `0..=64`.
    fn write_bits(&mut self, word: Word, bits: usize);
    /// Writes `bytes`.
    fn write_bytes(&mut self, bytes: &[u8]);

    /// Writes `false`. Might be faster than `writer.write_bit(false)`.
    #[inline(always)]
    fn write_false(&mut self) {
        self.write_zeros(1);
    }
    /// Writes up to 64 zero bits. Might be faster than `writer.write_bits(0, bits)`.
    fn write_zeros(&mut self, bits: usize) {
        self.write_bits(0, bits);
    }

    /// Number of bits that were written to the Writer
    fn num_bits_written(&self) -> usize;
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::bit_buffer::BitBuffer;
    use crate::buffer::BufferTrait;
    use crate::word_buffer::WordBuffer;
    use paste::paste;
    use test::{black_box, Bencher};

    // How many times each benchmark calls the function.
    const TIMES: usize = 1000;

    #[bench]
    fn bench_vec(b: &mut Bencher) {
        let mut vec = vec![];
        b.iter(|| {
            let vec = black_box(&mut vec);
            vec.clear();
            for _ in 0..TIMES {
                vec.push(black_box(0b10101u8))
            }
            black_box(vec);
        });
    }

    fn bench_write_bit<T: BufferTrait>(b: &mut Bencher) {
        let mut buf = T::default();
        b.iter(|| {
            let buf = black_box(&mut buf);
            let mut writer = buf.start_write();
            for _ in 0..TIMES {
                writer.write_bit(black_box(true))
            }
            buf.finish_write(writer);
        });
    }

    fn bench_write_bytes<T: BufferTrait>(b: &mut Bencher, bytes: usize) {
        let v = vec![123u8; bytes];
        let mut buf = T::default();
        b.iter(|| {
            let buf = black_box(&mut buf);
            let mut writer = buf.start_write();
            writer.write_bit(true); // Make write_bytes unaligned.
            for _ in 0..TIMES {
                writer.write_bytes(black_box(v.as_slice()))
            }
            buf.finish_write(writer);
        });
    }

    fn bench_write_bytes_range<T: BufferTrait>(b: &mut Bencher, min: usize, max: usize) {
        use rand::prelude::*;

        let mut rng = rand_chacha::ChaCha20Rng::from_seed(Default::default());
        let v: Vec<Vec<_>> = (0..TIMES)
            .map(|_| (0..rng.gen_range(min..=max)).map(|i| i as u8).collect())
            .collect();

        let mut buf = T::default();
        b.iter(|| {
            let buf = black_box(&mut buf);
            let mut writer = buf.start_write();
            writer.write_bit(true); // Make write_bytes unaligned.
            for v in black_box(v.as_slice()) {
                writer.write_bytes(v)
            }
            buf.finish_write(writer);
        });
    }

    fn bench_write_bits<T: BufferTrait>(b: &mut Bencher, bits: usize) {
        let v = Word::MAX >> (Word::BITS as usize - bits);
        let mut buf = T::default();
        b.iter(|| {
            let buf = black_box(&mut buf);
            let mut writer = buf.start_write();
            for _ in 0..TIMES {
                writer.write_bits(black_box(v), black_box(bits))
            }
            buf.finish_write(writer);
        });
    }

    #[bench]
    fn bench_word_buffer_write_false(b: &mut Bencher) {
        let mut buf = WordBuffer::default();
        b.iter(|| {
            let buf = black_box(&mut buf);
            let mut writer = buf.start_write();
            for _ in 0..TIMES {
                writer.write_false()
            }
            buf.finish_write(writer);
        });
    }

    macro_rules! bench_write_bits {
        ($name:ident, $T:ty, $n:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _write_bits_ $n>](b: &mut Bencher) {
                    bench_write_bits::<$T>(b, $n);
                }
            }
        };
    }

    macro_rules! bench_write_bytes {
        ($name:ident, $T:ty, $n:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _write_bytes_ $n>](b: &mut Bencher) {
                    bench_write_bytes::<$T>(b, $n);
                }
            }
        };
    }

    macro_rules! bench_write_bytes_range {
        ($name:ident, $T:ty, $min:literal, $max:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _write_bytes_ $min _to_ $max>](b: &mut Bencher) {
                    bench_write_bytes_range::<$T>(b, $min, $max);
                }
            }
        };
    }

    macro_rules! bench_write {
        ($name:ident, $T:ty) => {
            paste! {
                #[bench]
                fn [<bench_ $name _write_bit1>](b: &mut Bencher) {
                    bench_write_bit::<$T>(b);
                }
            }

            bench_write_bits!($name, $T, 5);
            bench_write_bits!($name, $T, 41);
            bench_write_bytes!($name, $T, 1);
            bench_write_bytes!($name, $T, 10);
            bench_write_bytes!($name, $T, 100);
            bench_write_bytes!($name, $T, 1000);

            bench_write_bytes_range!($name, $T, 0, 8);
            bench_write_bytes_range!($name, $T, 0, 16);
        };
    }

    bench_write!(bit_buffer, BitBuffer);
    bench_write!(word_buffer, WordBuffer);
}
