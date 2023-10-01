use std::num::NonZeroUsize;

use anyhow::Result;
use bitcode::word::Word;
use serde::de::DeserializeOwned;

pub trait ReadBuffer {
    fn capacity(&self) -> usize;

    /// Deserialize from the buffer into a value
    fn deserialize<T: DeserializeOwned>(&mut self) -> Result<T>;

    /// Copy the bytes into the buffer, so that we can deserialize them
    fn start_read(bytes: &[u8]) -> Self;

    /// Check for errors such as Eof and ExpectedEof
    fn finish_read(&mut self) -> Result<()>;
}

/// Abstracts over reading bits from a buffer.
pub trait BitRead {
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
