use bitcode::buffer::BufferTrait;
use bitcode::read::Read;
use bitcode::word_buffer::WordBuffer;
use bitcode::Buffer;
use std::num::NonZeroUsize;

pub(crate) struct ReadBuffer<'a> {
    pub(crate) buffer: Buffer,
    pub(crate) reader: <WordBuffer as BufferTrait>::Reader<'a>,
    pub(crate) context: <WordBuffer as BufferTrait>::Context,
}

impl ReadBuffer<'_> {
    /// From a slice of bytes

    pub(crate) fn build_from_bytes(bytes: &[u8]) -> Self {
        let mut buffer = Buffer::with_capacity(bytes.len());
        let (reader, context) = buffer.0.start_read(bytes);
        Self {
            buffer,
            reader,
            context,
        }
    }
}

impl Read for ReadBuffer<'_> {
    fn advance(&mut self, bits: usize) {
        self.reader.advance(bits)
    }

    fn peek_bits(&mut self) -> bitcode::Result<bitcode::word::Word> {
        self.reader.peek_bits()
    }

    fn read_bit(&mut self) -> bitcode::Result<bool> {
        self.reader.read_bit()
    }

    fn read_bits(&mut self, bits: usize) -> bitcode::Result<bitcode::word::Word> {
        self.reader.read_bits(bits)
    }

    fn read_bytes(&mut self, len: NonZeroUsize) -> bitcode::Result<&[u8]> {
        self.reader.read_bytes(len)
    }

    fn reserve_bits(&self, bits: usize) -> bitcode::Result<()> {
        self.reader.reserve_bits(bits)
    }
}
