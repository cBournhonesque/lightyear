use bitcode::encoding::Encoding;
use bitcode::Encode;
use serde::Serialize;

/// Buffer to facilitate writing bits
pub trait WriteBuffer {
    /// Serialize the given value into the buffer
    /// There is no padding when we serialize a value (i.e. it's possible to add a single bit
    /// to the buffer)

    fn serialize<T: Serialize + ?Sized>(&mut self, t: &T) -> anyhow::Result<()>;

    fn encode<T: Encode + ?Sized>(&mut self, t: &T, encoding: impl Encoding) -> anyhow::Result<()>;

    fn with_capacity(capacity: usize) -> Self;

    /// Clears the buffer.
    fn start_write(&mut self);

    /// Returns the finalized bytes (with padding to make a full byte)
    /// There is 0-7 bits of padding so that the serialized value is byte-aligned
    fn finish_write(&mut self) -> &[u8];

    fn num_bits_written(&self) -> usize;
    fn overflowed(&self) -> bool;

    /// Increase the maximum number of bits that can be written with this buffer
    /// (we are tracking them separately from the buffers capacity)
    fn reserve_bits(&mut self, num_bits: usize);

    /// Decrease the maximum number of bits that can be written with this buffer
    fn release_bits(&mut self, num_bits: usize);

    /// Set the number of bits that can be written to this buffer
    fn set_reserved_bits(&mut self, num_bits: usize);
}
