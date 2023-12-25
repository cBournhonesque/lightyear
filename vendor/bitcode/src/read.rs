use crate::word::Word;
use crate::Result;
use std::num::NonZeroUsize;

/// Abstracts over reading bits from a buffer.
pub trait Read {
    /// Advances any amount of bits. Must never fail.
    fn advance(&mut self, bits: usize);
    /// Peeks 64 bits without reading them. Bits after EOF are zeroed.
    fn peek_bits(&mut self) -> Result<Word>;
    // Reads 1 bit.
    fn read_bit(&mut self) -> Result<bool>;
    /// Reads up to 64 bits. `bits` must be in range `1..=64`.
    fn read_bits(&mut self, bits: usize) -> Result<Word>;
    /// Reads `len` bytes.
    fn read_bytes(&mut self, len: NonZeroUsize) -> Result<&[u8]>;
    /// Ensures that at least `bits` remain. Never underreports remaining bits.
    fn reserve_bits(&self, bits: usize) -> Result<()>;
}

#[cfg(all(test, not(miri)))]
mod tests {
    use crate::bit_buffer::BitBuffer;
    use crate::buffer::BufferTrait;
    use crate::nightly::div_ceil;
    use crate::read::Read;
    use crate::word_buffer::WordBuffer;
    use paste::paste;
    use std::num::NonZeroUsize;
    use test::{black_box, Bencher};

    fn bench_start_read<T: BufferTrait>(b: &mut Bencher) {
        let bytes = vec![123u8; 6659];
        let mut buf = T::default();

        b.iter(|| {
            black_box(buf.start_read(black_box(bytes.as_slice())));
        });
    }

    // How many times each benchmark calls the function.
    const TIMES: usize = 1000;

    fn bench_read_bit<T: BufferTrait>(b: &mut Bencher) {
        let bytes = vec![123u8; div_ceil(TIMES, u8::BITS as usize)];
        let mut buf = T::default();
        let _ = buf.start_read(&bytes);

        b.iter(|| {
            let buf = black_box(&mut buf);
            let (mut reader, _) = buf.start_read(black_box(&bytes));
            for _ in 0..black_box(TIMES) {
                black_box(reader.read_bit().unwrap());
            }
        });
    }

    fn bench_read_bits<T: BufferTrait>(b: &mut Bencher, bits: usize) {
        let bytes = vec![123u8; div_ceil(bits * TIMES, u8::BITS as usize)];
        let mut buf = T::default();
        let _ = buf.start_read(&bytes);

        b.iter(|| {
            let buf = black_box(&mut buf);
            let (mut reader, _) = buf.start_read(black_box(&bytes));
            for _ in 0..black_box(TIMES) {
                black_box(reader.read_bits(bits).unwrap());
            }
        });
    }

    fn bench_read_bytes<T: BufferTrait>(b: &mut Bencher, byte_count: usize) {
        let bytes = vec![123u8; byte_count * TIMES + 1];
        let mut buf = T::default();
        let _ = buf.start_read(&bytes);

        let byte_count = NonZeroUsize::new(byte_count).expect("must be >= 0");
        b.iter(|| {
            let buf = black_box(&mut buf);
            let (mut reader, _) = buf.start_read(black_box(&bytes));
            reader.read_bit().unwrap(); // Make read_bytes unaligned.
            for _ in 0..black_box(TIMES) {
                black_box(reader.read_bytes(byte_count).unwrap());
            }
        });
    }

    fn random_lengths(min: NonZeroUsize, max: NonZeroUsize) -> Vec<NonZeroUsize> {
        use rand::prelude::*;
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(Default::default());

        (0..TIMES)
            .map(|_| NonZeroUsize::new(rng.gen_range(min.get()..=max.get())).unwrap())
            .collect()
    }

    fn bench_read_bytes_range<T: BufferTrait>(b: &mut Bencher, min: usize, max: usize) {
        let min = NonZeroUsize::new(min).expect("must be >= 0");
        let max = NonZeroUsize::new(max).expect("must be >= 0");

        let lens = random_lengths(min, max);
        let total_len: usize = lens.iter().map(|l| l.get()).sum();
        let bytes = vec![123u8; total_len + 1];

        let mut buf = T::default();
        let _ = buf.start_read(&bytes);

        b.iter(|| {
            let buf = black_box(&mut buf);
            let (mut reader, _) = buf.start_read(black_box(&bytes));
            reader.read_bit().unwrap(); // Make read_bytes unaligned.
            for &len in black_box(lens.as_slice()) {
                black_box(reader.read_bytes(len).unwrap());
            }
        });
    }

    macro_rules! bench_read_bits {
        ($name:ident, $T:ty, $n:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _read_bits_ $n>](b: &mut Bencher) {
                    bench_read_bits::<$T>(b, $n);
                }
            }
        };
    }

    macro_rules! bench_read_bytes {
        ($name:ident, $T:ty, $n:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _read_bytes_ $n>](b: &mut Bencher) {
                    bench_read_bytes::<$T>(b, $n);
                }
            }
        };
    }

    macro_rules! bench_read_bytes_range {
        ($name:ident, $T:ty, $min:literal, $max:literal) => {
            paste! {
                #[bench]
                fn [<bench_ $name _read_bytes_ $min _to_ $max>](b: &mut Bencher) {
                    bench_read_bytes_range::<$T>(b, $min, $max);
                }
            }
        };
    }

    macro_rules! bench_read {
        ($name:ident, $T:ty) => {
            paste! {
                #[bench]
                fn [<bench_ $name _copy_from_slice>](b: &mut Bencher) {
                    bench_start_read::<$T>(b);
                }

                #[bench]
                fn [<bench_ $name _read_bit1>](b: &mut Bencher) {
                    bench_read_bit::<$T>(b);
                }
            }

            bench_read_bits!($name, $T, 5);
            bench_read_bits!($name, $T, 41);
            bench_read_bytes!($name, $T, 1);
            bench_read_bytes!($name, $T, 10);
            bench_read_bytes!($name, $T, 100);
            bench_read_bytes!($name, $T, 1000);
            bench_read_bytes!($name, $T, 10000);

            bench_read_bytes_range!($name, $T, 1, 8);
            bench_read_bytes_range!($name, $T, 1, 16);
        };
    }

    bench_read!(bit_buffer, BitBuffer);
    bench_read!(word_buffer, WordBuffer);
}
