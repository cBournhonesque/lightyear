use serde::Serialize;

/// Buffer to facilitate writing bits
pub trait WriteBuffer {
    // type Writer: BitWrite;

    /// Serialize the given value into the buffer
    /// There is 0-7 bits of padding so that the serialized value is byte-aligned
    fn serialize<T: Serialize + ?Sized>(&mut self, t: &T) -> anyhow::Result<()>;

    fn capacity(&self) -> usize;
    fn with_capacity(capacity: usize) -> Self;

    /// Clears the buffer.
    fn start_write(&mut self);

    /// Returns the finalized bytes (with padding to make a full byte)
    fn finish_write(&mut self) -> &[u8];

    fn num_bits_written(&self) -> usize;

    /// Increase the maximum number of bits that can be written with this buffer
    /// (this is separate from the buffers capacity)
    fn reserve_bits(&mut self, num_bits: u32);

    /// Decrease the maximum number of bits that can be written with this buffer
    fn release_bits(&mut self, num_bits: u32);
}

pub trait BitWrite {
    fn write_bit(&mut self, bit: bool);
    fn write_bits(&mut self, bits: u32);
    fn write_bytes(&mut self, bytes: &[u8]);
    fn num_bits_written(&self);
}
