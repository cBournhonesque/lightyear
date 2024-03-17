use anyhow::Context;
use bitcode::buffer::BufferTrait;
use bitcode::encoding::{Encoding, Fixed};
use bitcode::word_buffer::{WordBuffer, WordWriter};
use bitcode::write::Write;
use bitcode::Encode;
use serde::Serialize;

use crate::serialize::writer::WriteBuffer;

// strategy for message/channels

// we have a custom Serializer/Deserializer (that wraps around the bitcode serializer/deserializer)
// and contains a reference to MessageRegistry, or ChannelRegistry

// during #[derive(Message)]
// we generate a custom encode/decode
// most of the other subfields are Serializable

#[derive(Default)]
pub struct WriteWordBuffer {
    pub(crate) buffer: WordBuffer,
    pub(crate) writer: WordWriter,
    max_bits: usize,
}

#[derive(Encode, Serialize)]
// #[bitcode_hint(gamma)]
struct OnlyGammaEncode<'a, T: Serialize + ?Sized>(#[bitcode(with_serde)] &'a T);

impl WriteBuffer for WriteWordBuffer {
    // fn serialize<T: Serialize + ?Sized>(&mut self, t: &T) -> anyhow::Result<()> {
    //     serialize_compat(t, Fixed, &mut self.writer).context("error serializing")
    // }

    // TODO: define actual error types so users can distinguish between the two
    fn serialize<T: Serialize + ?Sized>(&mut self, t: &T) -> anyhow::Result<()> {
        let with_gamma = OnlyGammaEncode::<T>(t);
        with_gamma
            .encode(Fixed, &mut self.writer)
            .context("error serializing")
        // if self.overflowed() {
        //     bail!("buffer overflowed")
        // }
    }

    fn encode<T: Encode + ?Sized>(&mut self, t: &T, encoding: impl Encoding) -> anyhow::Result<()> {
        t.encode(encoding, &mut self.writer)
            .context("error encoding")
    }

    fn with_capacity(capacity: usize) -> Self {
        let mut buffer = WordBuffer::with_capacity(capacity);
        let writer = buffer.start_write();
        Self {
            buffer,
            writer,
            max_bits: 0,
        }
    }

    /// Reset the buffer to be empty (without any allocation)
    fn start_write(&mut self) {
        self.writer = self.buffer.start_write();
    }

    fn finish_write(&mut self) -> &[u8] {
        self.buffer.finish_write(std::mem::take(&mut self.writer))
    }
    fn num_bits_written(&self) -> usize {
        self.writer.num_bits_written()
    }

    fn overflowed(&self) -> bool {
        self.num_bits_written() > self.max_bits
    }

    fn reserve_bits(&mut self, num_bits: usize) {
        self.max_bits = self.max_bits.saturating_sub(num_bits);
    }

    fn release_bits(&mut self, num_bits: usize) {
        self.max_bits += num_bits;
    }

    fn set_reserved_bits(&mut self, num_bits: usize) {
        self.max_bits = num_bits;
    }
}

#[cfg(test)]
mod tests {
    use crate::serialize::reader::ReadBuffer;
    use crate::serialize::wordbuffer::reader::ReadWordBuffer;

    #[test]
    fn test_write_bits() -> anyhow::Result<()> {
        use super::*;
        use crate::serialize::writer::WriteBuffer;

        let mut buffer = WriteWordBuffer::with_capacity(5);
        // confirm that we serialize bit by bit
        buffer.serialize(&true)?;
        buffer.serialize(&false)?;
        buffer.serialize(&true)?;
        buffer.serialize(&true)?;
        // finish
        let bytes = buffer.finish_write();

        // in little-endian, we write the bits in reverse order
        assert_eq!(bytes, &[0b00001101]);

        let mut read_buffer = ReadWordBuffer::start_read(bytes);
        let bool = read_buffer.deserialize::<bool>()?;
        assert!(bool);
        let bool = read_buffer.deserialize::<bool>()?;
        assert!(!bool);
        let bool = read_buffer.deserialize::<bool>()?;
        assert!(bool);
        let bool = read_buffer.deserialize::<bool>()?;
        assert!(bool);
        read_buffer.finish_read()?;

        dbg!(bytes);
        Ok(())
    }

    #[test]
    fn test_write_multiple_objects() -> anyhow::Result<()> {
        use super::*;
        use crate::serialize::writer::WriteBuffer;
        use serde::Serialize;

        let mut buffer = WriteWordBuffer::with_capacity(2);
        let first_vec: Vec<u32> = vec![4, 6, 3];
        let second_vec: Vec<u64> = vec![2, 5];
        // confirm that we serialize bit by bit
        buffer.serialize(&first_vec)?;
        buffer.serialize(&second_vec)?;
        // finish
        let bytes = buffer.finish_write();

        let mut read_buffer = ReadWordBuffer::start_read(bytes);
        let vec = read_buffer.deserialize::<Vec<u32>>()?;
        assert_eq!(vec, first_vec);
        let vec = read_buffer.deserialize::<Vec<u64>>()?;
        assert_eq!(vec, second_vec);
        read_buffer.finish_read()?;

        Ok(())
    }

    // #[test]
    // fn test_write_gamma() -> anyhow::Result<()> {
    //     use super::*;
    //     use crate::serialize::writer::WriteBuffer;
    //     use serde::Serialize;
    //
    //     let mut buffer = WriteWordBuffer::with_capacity(10);
    //     buffer.serialize(&7_i64)?;
    //     let bytes = buffer.finish_write();
    //     assert_eq!(bytes.len(), 1);
    //     let mut read_buffer = ReadWordBuffer::start_read(bytes);
    //     let val = read_buffer.deserialize::<i64>()?;
    //     assert_eq!(val, 7_i64);
    //
    //     Ok(())
    // }
}
