use crate::serialize::writer::{BitWrite, WriteBuffer};
use anyhow::Context;
use bitcode::buffer::BufferTrait;
use bitcode::encoding::Fixed;
use bitcode::serde::ser::serialize_compat;
use bitcode::word_buffer::{WordBuffer, WordWriter};
use bitcode::write::Write;
use serde::Serialize;

#[derive(Default)]
pub(crate) struct WriteWordBuffer {
    pub(crate) buffer: WordBuffer,
    pub(crate) writer: WordWriter,
}

impl WriteBuffer for WriteWordBuffer {
    fn serialize<T: Serialize + ?Sized>(&mut self, t: &T) -> anyhow::Result<()> {
        serialize_compat(t, Fixed, &mut self.writer).context("error serializing")
    }

    fn capacity(&self) -> usize {
        self.buffer.capacity()
    }

    fn with_capacity(capacity: usize) -> Self {
        let mut buffer = WordBuffer::with_capacity(capacity);
        let writer = buffer.start_write();
        Self { buffer, writer }
    }

    fn start_write(&mut self) {
        self.writer = self.buffer.start_write();
    }

    fn finish_write(&mut self) -> &[u8] {
        self.buffer.finish_write(std::mem::take(&mut self.writer))
    }
    fn num_bits_written(&self) -> usize {
        self.writer.num_bits_written()
    }

    fn reserve_bits(&mut self, num_bits: u32) {
        todo!()
    }

    fn release_bits(&mut self, num_bits: u32) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use crate::serialize::wordbuffer::writer::WriteWordBuffer;

    #[test]
    fn test_write() -> anyhow::Result<()> {
        use super::*;
        use crate::serialize::writer::WriteBuffer;
        use bitcode::word::Word;
        use bitcode::word_buffer::WordBuffer;
        use bitcode::write::Write;
        use serde::Serialize;

        let mut buffer = WriteWordBuffer::with_capacity(5);
        buffer.serialize(&true)?;
        buffer.serialize(&false)?;
        let bytes = buffer.finish_write();

        dbg!(bytes);
        Ok(())
    }
}
