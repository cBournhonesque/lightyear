use crate::buffer::BufferTrait;
use crate::encoding::ByteEncoding;
use crate::read::Read;
use crate::word::*;
use crate::write::Write;
use crate::{Result, E};
use bitvec::domain::Domain;
use bitvec::prelude::*;
use std::num::NonZeroUsize;

/// A slow proof of concept [`Buffer`] that uses [`BitVec`]. Useful for comparison.
#[derive(Debug, Default)]
pub struct BitBuffer {
    bits: BitVec<u8, Lsb0>,
    read_bytes_buf: Box<[u8]>,
}

impl BufferTrait for BitBuffer {
    type Writer = BitWriter;
    type Reader<'a> = BitReader<'a>;
    type Context = ();

    fn capacity(&self) -> usize {
        self.bits.capacity() / u8::BITS as usize
    }

    fn with_capacity(cap: usize) -> Self {
        Self {
            bits: BitVec::with_capacity(cap * u8::BITS as usize),
            ..Default::default()
        }
    }

    fn start_write(&mut self) -> Self::Writer {
        self.bits.clear();
        Self::Writer {
            bits: std::mem::take(&mut self.bits),
        }
    }

    fn finish_write(&mut self, writer: Self::Writer) -> &[u8] {
        let Self::Writer { bits } = writer;
        self.bits = bits;

        self.bits.force_align();
        self.bits.as_raw_slice()
    }

    fn start_read<'a>(&'a mut self, bytes: &'a [u8]) -> (Self::Reader<'a>, Self::Context) {
        let bits = BitSlice::from_slice(bytes);
        let reader = Self::Reader {
            bits,
            read_bytes_buf: &mut self.read_bytes_buf,
            advanced_too_far: false,
        };

        (reader, ())
    }

    fn finish_read(reader: Self::Reader<'_>, _: Self::Context) -> Result<()> {
        if reader.advanced_too_far {
            return Err(E::Eof.e());
        }

        if reader.bits.is_empty() {
            return Ok(());
        }

        // Make sure no trailing 1 bits or zero bytes.
        let e = match reader.bits.domain() {
            Domain::Enclave(e) => e,
            Domain::Region { head, body, tail } => {
                if !body.is_empty() {
                    return Err(E::ExpectedEof.e());
                }
                head.xor(tail).ok_or_else(|| E::ExpectedEof.e())?
            }
        };
        (e.into_bitslice().count_ones() == 0)
            .then_some(())
            .ok_or_else(|| E::ExpectedEof.e())
    }
}

pub struct BitWriter {
    bits: BitVec<u8, Lsb0>,
}

impl Write for BitWriter {
    fn write_bit(&mut self, v: bool) {
        self.bits.push(v);
    }

    fn write_bits(&mut self, word: Word, bits: usize) {
        self.bits
            .extend_from_bitslice(&BitSlice::<u8, Lsb0>::from_slice(&word.to_le_bytes())[..bits]);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.bits
            .extend_from_bitslice(BitSlice::<u8, Lsb0>::from_slice(bytes));
    }

    fn num_bits_written(&self) -> usize {
        self.bits.len()
    }
}

pub struct BitReader<'a> {
    bits: &'a BitSlice<u8, Lsb0>,
    read_bytes_buf: &'a mut Box<[u8]>,
    advanced_too_far: bool,
}

impl BitReader<'_> {
    fn read_slice(&mut self, bits: usize) -> Result<&BitSlice<u8, Lsb0>> {
        if bits > self.bits.len() {
            return Err(E::Eof.e());
        }

        let (slice, remaining) = self.bits.split_at(bits);
        self.bits = remaining;
        Ok(slice)
    }
}

impl Read for BitReader<'_> {
    fn advance(&mut self, bits: usize) {
        if bits > self.bits.len() {
            // Handle the error later since we can't return it.
            self.advanced_too_far = true;
        }
        self.bits = &self.bits[bits.min(self.bits.len())..];
    }

    fn peek_bits(&mut self) -> Result<Word> {
        if self.advanced_too_far {
            return Err(E::Eof.e());
        }

        let bits = self.bits.len().min(64);

        let mut v = [0; 8];
        BitSlice::<u8, Lsb0>::from_slice_mut(&mut v)[..bits].copy_from_bitslice(&self.bits[..bits]);
        Ok(Word::from_le_bytes(v))
    }

    fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read_slice(1)?[0])
    }

    fn read_bits(&mut self, bits: usize) -> Result<Word> {
        let slice = self.read_slice(bits)?;

        let mut v = [0; 8];
        BitSlice::<u8, Lsb0>::from_slice_mut(&mut v)[..bits].copy_from_bitslice(slice);
        Ok(Word::from_le_bytes(v))
    }

    fn read_bytes(&mut self, len: NonZeroUsize) -> Result<&[u8]> {
        let len = len.get();

        // Take to avoid borrowing issue.
        let mut tmp = std::mem::take(self.read_bytes_buf);

        let bits = len
            .checked_mul(u8::BITS as usize)
            .ok_or_else(|| E::Eof.e())?;
        let slice = self.read_slice(bits)?;

        // Only allocate after reserve_read to prevent memory exhaustion attacks.
        if tmp.len() < len {
            tmp = vec![0; len.next_power_of_two()].into_boxed_slice()
        }

        tmp.as_mut_bits()[..slice.len()].copy_from_bitslice(slice);
        *self.read_bytes_buf = tmp;
        Ok(&self.read_bytes_buf[..len])
    }

    fn reserve_bits(&self, bits: usize) -> Result<()> {
        if bits <= self.bits.len() {
            Ok(())
        } else {
            Err(E::Eof.e())
        }
    }
}
