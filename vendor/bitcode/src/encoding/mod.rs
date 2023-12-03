use crate::{Decode, Encode};
use prelude::*;
use std::num::NonZeroUsize;

#[cfg(all(feature = "simdutf8", not(miri)))]
use simdutf8::basic::from_utf8;
#[cfg(not(all(feature = "simdutf8", not(miri))))]
use std::str::from_utf8;

mod bit_string;
mod expect_normalized_float;
mod expected_range_u64;
mod gamma;
mod prelude;

pub use bit_string::*;
pub use expect_normalized_float::ExpectNormalizedFloat;
pub use expected_range_u64::ExpectedRangeU64;
pub use gamma::Gamma;

pub trait Encoding: Copy {
    fn is_fixed(self) -> bool {
        false
    }

    fn zigzag(self) -> bool {
        false
    }

    #[inline(always)]
    fn write_u64<const BITS: usize>(self, writer: &mut impl Write, v: u64) {
        writer.write_bits(v, BITS);
    }

    #[inline(always)]
    fn read_u64<const BITS: usize>(self, reader: &mut impl Read) -> Result<u64> {
        reader.read_bits(BITS)
    }

    // TODO add implementations to Gamma and ExpectedRange.
    #[inline(always)]
    fn write_u128<const BITS: usize>(self, writer: &mut impl Write, v: u128) {
        debug_assert!((65..=128).contains(&BITS));

        let lo = v as u64;
        let hi = (v >> 64) as u64;
        writer.write_bits(lo, 64);
        writer.write_bits(hi, BITS - 64);
    }

    #[inline(always)]
    fn read_u128<const BITS: usize>(self, reader: &mut impl Read) -> Result<u128> {
        debug_assert!((65..=128).contains(&BITS));

        let lo = reader.read_bits(64)?;
        let hi = reader.read_bits(BITS - 64)?;
        Ok(lo as u128 | ((hi as u128) << 64))
    }

    #[inline(always)]
    fn write_f32(self, writer: &mut impl Write, v: f32) {
        v.to_bits().encode(Fixed, writer).unwrap()
    }

    #[inline(always)]
    fn read_f32(self, reader: &mut impl Read) -> Result<f32> {
        Ok(f32::from_bits(Decode::decode(Fixed, reader)?))
    }

    #[inline(always)]
    fn write_f64(self, writer: &mut impl Write, v: f64) {
        v.to_bits().encode(Fixed, writer).unwrap()
    }

    #[inline(always)]
    fn read_f64(self, reader: &mut impl Read) -> Result<f64> {
        Ok(f64::from_bits(Decode::decode(Fixed, reader)?))
    }

    #[inline(always)]
    fn write_str(self, writer: &mut impl Write, v: &str) {
        self.write_byte_str(writer, v.as_bytes());
    }

    #[inline(always)]
    fn read_str(self, reader: &mut impl Read) -> Result<&str> {
        let len = usize::decode(Gamma, reader)?;
        if let Some(len) = NonZeroUsize::new(len) {
            from_utf8(self.read_bytes(reader, len)?).map_err(|_| E::Invalid("utf8").e())
        } else {
            Ok("")
        }
    }

    #[inline(always)]
    fn write_byte_str(self, writer: &mut impl Write, v: &[u8]) {
        v.len().encode(Gamma, writer).unwrap();
        writer.write_bytes(v);
    }

    #[inline(always)]
    fn read_byte_str(self, reader: &mut impl Read) -> Result<&[u8]> {
        let len = usize::decode(Gamma, reader)?;
        if let Some(len) = NonZeroUsize::new(len) {
            self.read_bytes(reader, len)
        } else {
            Ok(&[])
        }
    }

    #[inline(always)]
    fn read_bytes(self, reader: &mut impl Read, len: NonZeroUsize) -> Result<&[u8]> {
        reader.read_bytes(len)
    }
}

#[derive(Copy, Clone)]
pub struct Fixed;

impl Encoding for Fixed {
    fn is_fixed(self) -> bool {
        true
    }
}
