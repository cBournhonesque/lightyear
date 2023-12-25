use crate::encoding::prelude::*;
use crate::encoding::Fixed;
use std::num::NonZeroUsize;

mod ascii;
mod ascii_lowercase;
mod bit_utils;

pub use ascii::Ascii;
pub use ascii_lowercase::AsciiLowercase;

/// Encodes strings with character sizes other than 8 bits (e.g. Ascii).
#[derive(Copy, Clone)]
pub struct BitString<C: ByteEncoding>(pub C);

impl<C: ByteEncoding> Encoding for BitString<C> {
    #[inline(always)]
    fn write_byte_str(self, _writer: &mut impl Write, _bytes: &[u8]) {
        unimplemented!()
        // let n = bytes.len();
        // n.encode(Gamma, writer).unwrap();
        // if n == 0 {
        //     return;
        // }
        //
        // let revert = writer.get_revert();
        // writer.write_false();
        // let is_valid = writer.write_encoded_bytes::<C>(bytes);
        //
        // if !is_valid {
        //     #[cold]
        //     fn cold<W: Write>(writer: &mut W, v: &[u8], revert: W::Revert) {
        //         writer.revert(revert);
        //         writer.write_bit(true);
        //         writer.write_bytes(v);
        //     }
        //     cold(writer, bytes, revert);
        // }
    }

    #[inline(always)]
    fn read_bytes(self, _reader: &mut impl Read, _len: NonZeroUsize) -> Result<&[u8]> {
        unimplemented!()
        // let is_valid = !reader.read_bit()?;
        // if is_valid {
        //     reader.read_encoded_bytes::<C>(len)
        // } else {
        //     #[cold]
        //     fn cold(reader: &mut impl Read, len: NonZeroUsize) -> Result<&[u8]> {
        //         reader.read_bytes(len)
        //     }
        //     cold(reader, len)
        // }
    }
}

/// A `u8` encoding for [`BitString`]. Each `u8` is encoded with a fixed number of bits
/// (e.g. Ascii = 7 bits).
pub trait ByteEncoding: Copy {
    const BITS_PER_BYTE: usize;

    /// Returns if the `word` of up to 8 characters valid. Only `bytes` bytes are included
    /// (the remaining are zeroed). `bytes` must be at least 1.
    fn validate(word: Word, bytes: usize) -> bool;

    /// Packs 8 bytes to 8 * [`Self::BITS_PER_BYTE`] bits. The returned extra bits must
    /// be zeroed.
    fn pack(word: Word) -> Word;

    /// Unpacks 8 * [`Self::BITS_PER_BYTE`] bits to 8 bytes. The inputted extra bits are
    /// undefined. The returned extra bytes are undefined.
    fn unpack(word: Word) -> Word;
}

// For benchmarking overhead of BitString. DO NOT USE
impl ByteEncoding for Fixed {
    const BITS_PER_BYTE: usize = 8;

    #[inline(always)]
    fn validate(_: Word, _: usize) -> bool {
        true
    }

    #[inline(always)]
    fn pack(word: Word) -> Word {
        word
    }

    #[inline(always)]
    fn unpack(word: Word) -> Word {
        word
    }
}

#[cfg(all(test, debug_assertions, not(miri)))]
mod tests {
    use super::*;
    use crate::encoding::prelude::test_prelude::*;
    use crate::encoding::BitString;

    #[test]
    fn test() {
        fn t<V: Encode + Decode + Clone + Debug + PartialEq>(value: V) {
            test_encoding(BitString(Ascii), value.clone());
            test_encoding(BitString(AsciiLowercase), value.clone());
            test_encoding(BitString(Fixed), value.clone());
            test_encoding(Fixed, value);
        }

        for i in 0..u8::MAX {
            t(i.to_string());
        }

        t("abcd123".repeat(10));
        t("hello".to_string());
        t("☺".to_string());

        #[derive(Encode, Copy, Clone)]
        struct AsciiString(#[bitcode_hint(ascii)] &'static str);
        #[derive(Encode, Copy, Clone)]
        struct AsciiLowercaseString(#[bitcode_hint(ascii_lowercase)] &'static str);

        let is_valid_bit = 1;

        // Is ascii (ascii is 2 bits shorter, ascii_lowercase is 8 bits shorter).
        let s = "foo";
        let len_bits = 5;
        assert_eq!(
            crate::encode(&[s; 8]).unwrap().len(),
            len_bits + s.len() * Fixed::BITS_PER_BYTE
        );
        assert_eq!(
            crate::encode(&[AsciiString(s); 8]).unwrap().len(),
            len_bits + is_valid_bit + s.len() * Ascii::BITS_PER_BYTE
        );
        assert_eq!(
            crate::encode(&[AsciiLowercaseString(s); 8]).unwrap().len(),
            len_bits + is_valid_bit + s.len() * AsciiLowercase::BITS_PER_BYTE
        );

        // Isn't ascii (both 1 bit longer output).
        let s = "☺☺☺";
        let len_bits = 7;
        assert_eq!(
            crate::encode(&[s; 8]).unwrap().len(),
            len_bits + s.len() * Fixed::BITS_PER_BYTE
        );
        assert_eq!(
            crate::encode(&[AsciiString(s); 8]).unwrap().len(),
            len_bits + is_valid_bit + s.len() * Fixed::BITS_PER_BYTE
        );
        assert_eq!(
            crate::encode(&[AsciiLowercaseString(s); 8]).unwrap().len(),
            len_bits + is_valid_bit + s.len() * Fixed::BITS_PER_BYTE
        );
    }
}

#[cfg(all(test, not(miri)))]
mod benches {
    use super::*;
    use crate::encoding::bench_prelude::*;

    fn string_dataset() -> Vec<String> {
        // return vec!["a".repeat(10000)];
        let max_size = 16;
        dataset::<u8>()
            .into_iter()
            .map(|n| "e".repeat(n as usize % (max_size + 1)))
            .collect()
    }

    mod ascii {
        use super::*;
        bench_encoding!(crate::encoding::BitString(Ascii), string_dataset);
    }

    mod ascii_lowercase {
        use super::*;
        bench_encoding!(crate::encoding::BitString(AsciiLowercase), string_dataset);
    }

    mod fixed {
        use super::*;
        bench_encoding!(crate::encoding::BitString(Fixed), string_dataset);
    }

    mod fixed_string {
        use super::*;
        bench_encoding!(crate::encoding::Fixed, string_dataset);
    }
}
