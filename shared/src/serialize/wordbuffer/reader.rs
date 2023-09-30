use crate::serialize::reader::ReadBuffer;
use anyhow::Context;
use bitcode::buffer::BufferTrait;
use bitcode::encoding::Fixed;
use bitcode::serde::de::deserialize_compat;
use bitcode::word_buffer::{WordBuffer, WordContext, WordReader};
use serde::de::DeserializeOwned;

pub(crate) struct ReadWordBuffer<'a> {
    pub(crate) buffer: WordBuffer,
    pub(crate) reader: Option<WordReader<'a>>,
    pub(crate) context: Option<WordContext>,
}

impl<'a> ReadBuffer for ReadWordBuffer<'a> {
    fn capacity(&self) -> usize {
        self.capacity()
    }

    fn deserialize<T: DeserializeOwned>(&mut self) -> anyhow::Result<T> {
        deserialize_compat(Fixed, &mut self.reader.unwrap()).context("error deserializing")
    }

    fn start_read(bytes: &[u8]) -> Self {
        let mut buffer = WordBuffer::with_capacity(bytes.len());
        let (reader, context) = buffer.start_read(&[]);
        Self {
            buffer,
            reader: Some(reader),
            context: Some(context),
        }
    }

    fn finish_read(&mut self) -> anyhow::Result<()> {
        let reader = std::mem::take(&mut self.reader).unwrap();
        let context = std::mem::take(&mut self.context).unwrap();
        WordBuffer::finish_read(reader, context).context("error finishing read")
    }
}
