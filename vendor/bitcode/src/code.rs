use crate::buffer::BufferTrait;
use crate::encoding::{Encoding, Fixed};
use crate::read::Read;
use crate::write::Write;
use crate::Result;

pub(crate) fn encode_internal<'a>(
    buffer: &'a mut impl BufferTrait,
    t: &(impl Encode + ?Sized),
) -> Result<&'a [u8]> {
    let mut writer = buffer.start_write();
    t.encode(Fixed, &mut writer)?;
    Ok(buffer.finish_write(writer))
}

pub(crate) fn decode_internal<B: BufferTrait, T: Decode>(
    buffer: &mut B,
    bytes: &[u8],
) -> Result<T> {
    let (mut reader, context) = buffer.start_read(bytes);
    let decode_result = T::decode(Fixed, &mut reader);
    B::finish_read_with_result(reader, context, decode_result)
}

/// A type which can be encoded to bytes with [`encode`][`crate::encode`].
///
/// Must use `#[derive(Encode)]` to implement.
/// ```
/// #[derive(bitcode::Encode)]
/// // If your struct contains itself you must annotate it with `#[bitcode(recursive)]`.
/// // This disables certain speed optimizations that aren't possible on recursive types.
/// struct MyStruct {
///     a: u32,
///     b: bool,
///     // If you want to use serde::Serialize on a field instead of bitcode::Encode.
///     #[cfg(feature = "serde")]
///     #[bitcode(with_serde)]
///     c: String,
/// }
/// ```
pub trait Encode {
    // The minimum and maximum number of bits a type can encode as. For now these are only valid if
    // the encoding is fixed. Before using them make sure the encoding passed to encode is fixed.
    // TODO make these const functions that take an encoding (once const fn is available in traits).
    #[doc(hidden)]
    const ENCODE_MIN: usize;

    // If max is lower than the actual max, we may not encode all the bits.
    #[doc(hidden)]
    const ENCODE_MAX: usize;

    #[doc(hidden)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()>;
}

/// A type which can be decoded from bytes with [`decode`][`crate::decode`].
///
/// Must use `#[derive(Decode)]` to implement.
/// ```
/// #[derive(bitcode::Decode)]
/// // If your struct contains itself you must annotate it with `#[bitcode(recursive)]`.
/// // This disables certain speed optimizations that aren't possible on recursive types.
/// struct MyStruct {
///     a: u32,
///     b: bool,
///     // If you want to use serde::Deserialize on a field instead of bitcode::Decode.
///     #[cfg(feature = "serde")]
///     #[bitcode(with_serde)]
///     c: String,
/// }
/// ```
pub trait Decode: Sized {
    // Copy of Encode constants. See Encode for details.
    // If min is higher than the actual min, we may get EOFs.
    #[doc(hidden)]
    const DECODE_MIN: usize;

    #[doc(hidden)]
    const DECODE_MAX: usize;

    #[doc(hidden)]
    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self>;
}

/// A macro that facilitates writing to a RegisterWriter when encoding multiple values less than 64 bits.
/// This can dramatically speed operations like encoding a tuple of 8 bytes.
///
/// Once you call `optimized_enc!()`, you must call `end_enc!()` at the end to flush the remaining bits.
/// If the execution path diverges it must never converge or this won't optimize well.
#[doc(hidden)]
#[macro_export]
macro_rules! optimized_enc {
    ($encoding:ident, $writer:ident) => {
        let mut buf = $crate::__private::RegisterWriter::new($writer);
        #[allow(unused_mut)]
        let mut i: usize = 0;
        #[allow(unused)]
        let no_encoding_upstream = $encoding.is_fixed();

        // Call on each field (that doesn't get it's encoding overridden in the derive macro).
        #[allow(unused)]
        macro_rules! enc {
            ($t:expr, $T:ty) => {
                // ENCODE_MAX is only accurate if there isn't any encoding upstream.
                // Downstream encodings make ENCODE_MAX = usize::MAX in derive macro.
                if <$T as $crate::__private::Encode>::ENCODE_MAX.saturating_add(i) <= 64
                    && no_encoding_upstream
                {
                    <$T as $crate::__private::Encode>::encode(&$t, $encoding, &mut buf.inner)?;
                } else {
                    if i != 0 {
                        buf.flush();
                    }

                    if <$T as $crate::__private::Encode>::ENCODE_MAX < 64 && no_encoding_upstream {
                        <$T as $crate::__private::Encode>::encode(&$t, $encoding, &mut buf.inner)?;
                    } else {
                        <$T as $crate::__private::Encode>::encode(&$t, $encoding, buf.writer)?;
                    }
                }

                i = if <$T as $crate::__private::Encode>::ENCODE_MAX.saturating_add(i) <= 64
                    && no_encoding_upstream
                {
                    <$T as $crate::__private::Encode>::ENCODE_MAX + i
                } else {
                    if <$T as $crate::__private::Encode>::ENCODE_MAX < 64 && no_encoding_upstream {
                        <$T as $crate::__private::Encode>::ENCODE_MAX
                    } else {
                        0
                    }
                };
            };
        }

        // Call to flush the contents of the RegisterWriter and get the inner writer.
        macro_rules! flush {
            () => {{
                if i != 0 {
                    buf.flush();
                }
                i = 0;
                &mut *buf.writer
            }};
        }

        // Call to encode an enum variant. Faster than flush!().write_bits($variant, $bits).
        // Must be fist call after optimized_enc!.
        #[allow(unused)]
        macro_rules! enc_variant {
            ($variant:literal, $bits:literal) => {
                debug_assert!(i == 0);
                buf.inner.write_bits($variant, $bits);
                i = $bits + i;
            };
        }

        // Call once done encoding.
        macro_rules! end_enc {
            () => {
                let _ = flush!();
                let _ = i;
                #[allow(clippy::drop_non_drop)]
                drop(buf);
            };
        }
    };
}
pub use optimized_enc;

// These benchmarks ensure that optimized_enc is working. They all run about 8 times faster with optimized_enc.
#[cfg(all(test, not(miri)))]
mod optimized_enc_tests {
    use std::collections::{BinaryHeap, VecDeque};
    use test::{black_box, Bencher};

    type A = u8;
    type B = u8;

    #[derive(Clone, Debug, PartialEq, crate::Encode, crate::Decode)]
    struct Foo {
        a: A,
        b: B,
    }

    #[bench]
    fn bench_foo(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = Foo { a: 1, b: 2 };
        let foo = vec![foo; 4000];

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: Vec<Foo> = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let foo = black_box(foo.as_slice());
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    #[bench]
    fn bench_tuple(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![(0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8); 1000];

        b.iter(|| {
            let foo = black_box(foo.as_slice());
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    #[bench]
    fn bench_array(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![[0u8; 8]; 1000];

        b.iter(|| {
            let foo = black_box(foo.as_slice());
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    #[bench]
    fn bench_bool_slice(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![false; 8 * 1000];

        b.iter(|| {
            let foo = black_box(foo.as_slice());
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    #[bench]
    fn bench_vec(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![0u8; 8 * 1000];

        b.iter(|| {
            let foo = black_box(foo.as_slice());
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    #[bench]
    fn bench_vec_deque(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let mut foo = VecDeque::from(vec![0u8; 8000]);
        for _ in 0..4000 {
            // Make it not contiguous.
            foo.pop_front().unwrap();
            foo.push_back(1u8);
        }

        b.iter(|| {
            let foo = black_box(&foo);
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }

    // BinaryHeap::encode isn't optimized yet.
    #[bench]
    fn bench_binary_heap(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = BinaryHeap::from_iter((0u16..8000).map(|v| v as u8));

        b.iter(|| {
            let foo = black_box(&foo);
            let bytes = buffer.encode(foo).unwrap();
            black_box(bytes);
        })
    }
}

/// A macro that facilitates reading from a RegisterReader when decoding multiple values less than 64 bits.
/// This can dramatically speed operations like decoding a tuple of 8 bytes.
///
/// Once you call `optimized_dec!()`, you must call `end_dec!()` at the end to advance the reader.
/// If the execution path diverges it must never converge or this won't optimize well.
#[doc(hidden)]
#[macro_export]
macro_rules! optimized_dec {
    ($encoding:ident, $reader:ident) => {
        #[allow(unused_mut)]
        let mut buf = $crate::__private::RegisterReader::new($reader);
        #[allow(unused_mut)]
        let mut i: usize = 0;
        #[allow(unused)]
        let no_encoding_upstream = $encoding.is_fixed();

        // Call on each field (that doesn't get it's encoding overridden in the derive macro).
        #[allow(unused)]
        macro_rules! dec {
            ($t:ident, $T:ty) => {
                // DECODE_MAX is only accurate if there isn't any encoding upstream.
                // Downstream encodings make DECODE_MAX = usize::MAX in derive macro.
                let $t = if i >= <$T as $crate::__private::Decode>::DECODE_MAX
                    && no_encoding_upstream
                {
                    <$T as $crate::__private::Decode>::decode($encoding, &mut buf.inner)?
                } else {
                    if <$T as $crate::__private::Decode>::DECODE_MAX < 64 && no_encoding_upstream {
                        buf.refill()?;
                        <$T as $crate::__private::Decode>::decode($encoding, &mut buf.inner)?
                    } else {
                        buf.advance_reader();
                        <$T as $crate::__private::Decode>::decode($encoding, buf.reader)?
                    }
                };

                i = if i >= <$T as $crate::__private::Decode>::DECODE_MAX && no_encoding_upstream {
                    i - <$T as $crate::__private::Decode>::DECODE_MAX
                } else {
                    if <$T as $crate::__private::Decode>::DECODE_MAX < 64 && no_encoding_upstream {
                        // Needs saturating since it's const (even though we've checked it).
                        64usize.saturating_sub(<$T as $crate::__private::Decode>::DECODE_MAX)
                    } else {
                        0
                    }
                };
            };
        }

        // Call to flush the contents of the RegisterReader and get the inner reader.
        macro_rules! flush {
            () => {{
                let _ = i;
                i = 0;
                buf.advance_reader();
                &mut *buf.reader
            }};
        }

        // Call to peek bits to decode an enum variant. Faster than flush!().peek_bits()?.
        // Must be fist call after optimized_dec!.
        #[allow(unused)]
        macro_rules! dec_variant_peek {
            () => {{
                buf.refill()?;
                buf.inner.peek_bits()?
            }};
        }

        // Call to advance bits to decode an enum variant. Faster than flush!().advance($bits)?.
        // Must be second call after dec_variant_peek!.
        #[allow(unused)]
        macro_rules! dec_variant_advance {
            ($bits:literal) => {
                debug_assert!(i == 0);
                buf.inner.advance($bits);
                i = 64 - $bits;
            };
        }

        // Call once done decoding.
        macro_rules! end_dec {
            () => {
                let _ = flush!();
                let _ = i;
                #[allow(clippy::drop_non_drop)]
                drop(buf);
            };
        }
    };
}
pub use optimized_dec;

// These benchmarks ensure that optimized_dec is working. They run 4-8 times faster with optimized_dec.
#[cfg(all(test, not(miri)))]
mod optimized_dec_tests {
    use std::collections::{BTreeSet, BinaryHeap, VecDeque};
    use test::{black_box, Bencher};

    type A = u8;
    type B = u8;

    #[derive(Clone, Debug, PartialEq, crate::Encode, crate::Decode)]
    #[repr(C, align(8))]
    struct Foo {
        a: A,
        b: B,
        c: A,
        d: B,
        e: A,
        f: B,
        g: A,
        h: B,
    }

    #[bench]
    fn bench_foo(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = Foo {
            a: 1,
            b: 2,
            c: 3,
            d: 4,
            e: 5,
            f: 6,
            g: 7,
            h: 8,
        };
        let foo = vec![foo; 1000];
        type T = Vec<Foo>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }

    #[bench]
    fn bench_tuple(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![(0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8); 1000];
        type T = Vec<(u8, u8, u8, u8, u8, u8, u8, u8)>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }

    #[bench]
    fn bench_array(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![[0u8; 8]; 1000];
        type T = Vec<[u8; 8]>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }

    #[bench]
    fn bench_vec(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = vec![0u8; 8000];
        type T = Vec<u8>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }

    #[bench]
    fn bench_vec_deque(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let mut foo = VecDeque::from(vec![0u8; 8000]);
        for _ in 0..4000 {
            // Make it not contiguous.
            foo.pop_front().unwrap();
            foo.push_back(1u8);
        }
        type T = VecDeque<u8>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();
        assert_eq!(foo, decoded);

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }

    #[bench]
    fn bench_binary_heap(b: &mut Bencher) {
        let mut buffer = crate::Buffer::new();
        let foo = BinaryHeap::from_iter((0u16..8000).map(|v| v as u8));
        type T = BinaryHeap<u8>;

        let bytes = buffer.encode(&foo).unwrap().to_vec();
        let decoded: T = buffer.decode(&bytes).unwrap();

        // Binary heaps can't be compared directly.
        assert_eq!(
            BTreeSet::from_iter(foo.iter().copied()),
            BTreeSet::from_iter(decoded.iter().copied())
        );

        b.iter(|| {
            let bytes = black_box(bytes.as_slice());
            black_box(buffer.decode::<T>(bytes).unwrap())
        })
    }
}
