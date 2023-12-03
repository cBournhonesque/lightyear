pub use crate::encoding::Encoding;
pub use crate::nightly::ilog2_u64;
pub use crate::read::Read;
pub use crate::word::*;
pub use crate::write::Write;
pub(crate) use crate::{Result, E};

#[cfg(all(test))]
pub mod test_prelude {
    pub use super::*;
    pub use crate::{Decode, Encode};
    pub use std::fmt::Debug;

    #[cfg(all(test, debug_assertions))]
    pub fn test_encoding_inner<
        B: crate::buffer::BufferTrait,
        V: Encode + Decode + Debug + PartialEq,
    >(
        encoding: impl Encoding,
        value: &V,
    ) {
        let mut buffer = B::default();

        let mut writer = buffer.start_write();
        value.encode(encoding, &mut writer).unwrap();
        let bytes = buffer.finish_write(writer).to_owned();

        let (mut reader, context) = buffer.start_read(&bytes);
        assert_eq!(&V::decode(encoding, &mut reader).unwrap(), value);
        B::finish_read(reader, context).unwrap();
    }

    #[cfg(all(test, debug_assertions))]
    pub fn test_encoding<V: Encode + Decode + Debug + PartialEq>(
        encoding: impl Encoding,
        value: V,
    ) {
        test_encoding_inner::<crate::bit_buffer::BitBuffer, V>(encoding, &value);
        test_encoding_inner::<crate::word_buffer::WordBuffer, V>(encoding, &value);
    }
}

#[cfg(test)]
pub mod bench_prelude {
    use super::test_prelude::*;
    use crate::buffer::BufferTrait;
    use crate::word_buffer::WordBuffer;
    use rand::distributions::Standard;
    use rand::prelude::*;
    use test::black_box;

    pub use super::*;
    pub use test::Bencher;

    pub fn dataset<T>() -> Vec<T>
    where
        Standard: Distribution<T>,
    {
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(Default::default());
        (0..1000).map(|_| rng.gen()).collect()
    }

    #[macro_export]
    macro_rules! bench_encoding {
        ($encoding:expr, $dataset:path) => {
            #[bench]
            fn encode(b: &mut Bencher) {
                bench_encode(b, $encoding, $dataset());
            }

            #[bench]
            fn decode(b: &mut Bencher) {
                bench_decode(b, $encoding, $dataset());
            }
        };
    }
    pub use bench_encoding;

    pub fn bench_encode(b: &mut Bencher, encoding: impl Encoding, data: Vec<impl Encode>) {
        let mut buf = WordBuffer::with_capacity(16000);
        let starting_cap = buf.capacity();

        b.iter(|| {
            let buf = black_box(&mut buf);
            let data = black_box(data.as_slice());

            let mut writer = buf.start_write();
            for v in data {
                v.encode(encoding, &mut writer).unwrap();
            }
            buf.finish_write(writer);
        });

        assert_eq!(buf.capacity(), starting_cap);
    }

    pub fn bench_decode<T: Encode + Decode + Debug + PartialEq>(
        b: &mut Bencher,
        encoding: impl Encoding,
        data: Vec<T>,
    ) {
        let mut buf = WordBuffer::default();

        let mut writer = buf.start_write();
        for v in &data {
            v.encode(encoding, &mut writer).unwrap();
        }
        let bytes = buf.finish_write(writer).to_owned();

        b.iter(|| {
            let buf = black_box(&mut buf);

            let (mut reader, _) = buf.start_read(black_box(bytes.as_slice()));
            for v in &data {
                let decoded = T::decode(encoding, &mut reader).unwrap();
                assert_eq!(&decoded, v);
            }
        })
    }
}
