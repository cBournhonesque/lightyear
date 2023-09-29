use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::word::Word;
use bitcode::word_buffer::{WordBuffer, WordWriter};
use bitcode::write::Write;
use bitcode::Buffer;
use std::num::NonZeroUsize;

#[derive(Default)]
pub(crate) struct WriteBuffer {
    pub(crate) buffer: Buffer,
    pub(crate) writer: WordWriter,
}

impl WriteBuffer {
    /// From a slice of bytes
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let mut buffer = Buffer::with_capacity(capacity);
        let writer = buffer.0.start_write();
        Self { buffer, writer }
    }
}

impl Write for WriteBuffer {
    fn write_bit(&mut self, v: bool) {
        self.writer.write_bit(v)
    }

    fn write_bits(&mut self, word: Word, bits: usize) {
        self.writer.write_bits(word, bits)
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.writer.write_bytes(bytes)
    }

    fn num_bits_written(&self) -> usize {
        self.writer.num_bits_written()
    }
}
