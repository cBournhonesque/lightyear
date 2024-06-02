use std::cell::UnsafeCell;

use crate::packet::packet::MTU_PAYLOAD_BYTES;
use bevy::ptr::UnsafeCellDeref;
use bitcode::buffer::BufferTrait;
use bitcode::encoding::{Encoding, Fixed};
use bitcode::word_buffer::{WordBuffer, WordContext, WordReader};
use bitcode::Decode;
use self_cell::self_cell;
use serde::de::DeserializeOwned;
use tracing::trace;

use crate::serialize::reader::ReadBuffer;

pub const READER_BUFFER_POOL_SIZE: usize = 1;

#[derive(Default)]
pub struct Reader<'a>(Option<(WordReader<'a>, WordContext)>);

#[derive(Decode)]
// #[bitcode_hint(gamma)]
struct OnlyGammaDecode<T: DeserializeOwned>(#[bitcode(with_serde)] T);

unsafe impl Send for BitcodeReader {}
unsafe impl Sync for BitcodeReader {}

pub(crate) struct BufferPool(pub(crate) crate::utils::pool::Pool<WordBuffer>);

fn new_buffer() -> WordBuffer {
    trace!("Allocating new buffer for ReadWordBuffer");
    WordBuffer::with_capacity(MTU_PAYLOAD_BYTES)
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(READER_BUFFER_POOL_SIZE)
    }
}

impl BufferPool {
    pub fn new(cap: usize) -> Self {
        Self(crate::utils::pool::Pool::new(cap, new_buffer))
    }

    pub fn start_read(&self, bytes: &[u8]) -> BitcodeReader {
        trace!("buffer pool length: {}", self.0.len());
        let buffer = self.0.pull(new_buffer);
        // let (reader, context) = buffer.start_read(bytes);
        // ReadWordBuffer { reader, context }
        let (_, buffer) = buffer.detach();
        BitcodeReader::start_read_with_buffer(bytes, buffer)
    }

    pub fn attach(&self, reader: BitcodeReader) {
        // return to the pool the buffer associated to the reader
        self.0.attach(reader.into_owner().into_inner());
    }
}

// pub struct ReadWordBuffer<'buffer> {
//     reader: WordReader<'buffer>,
//     context: WordContext,
// }

// We use self_cell because the reader contains a reference to the WordBuffer
// (it will take ownership of the buffer's contents to write into)
self_cell!(
    pub struct BitcodeReader {
        owner: UnsafeCell<WordBuffer>,
        #[covariant]
        // reader contains a reference to the buffer
        dependent: Reader,
    }
);

impl BitcodeReader {
    /// Copies the bytes into the internal buffer without allocating a new buffer
    pub(crate) fn reset_read(&mut self, bytes: &[u8]) {
        self.with_dependent_mut(|buffer, reader| {
            // SAFETY: we have mut access to the entire ReadWordBuffer
            unsafe {
                let (new_reader, context) = buffer.deref_mut().start_read(bytes);
                reader.0 = Some((new_reader, context));
            }
        });
    }
}

impl ReadBuffer for BitcodeReader {
    // fn deserialize<T: DeserializeOwned>(&mut self) -> anyhow::Result<T> {
    //     let with_gamma =
    //         OnlyGammaDecode::<T>::decode(Fixed, &mut self.reader).context("error deserializing")?;
    //     Ok(with_gamma.0)
    // }
    //
    // fn decode<T: Decode>(&mut self, encoding: impl Encoding) -> anyhow::Result<T> {
    //     T::decode(encoding, &mut self.reader).context("error decoding")
    // }
    //
    // fn start_read(bytes: &[u8]) -> Self {
    //     let mut buffer = WordBuffer::with_capacity(bytes.len());
    //     let (reader, context) = buffer.start_read(bytes);
    //     ReadWordBuffer { reader, context }
    // }
    //
    // fn finish_read(&mut self) -> anyhow::Result<()> {
    //     todo!();
    //     // WordBuffer::finish_read(self.reader, self.context).context("error finishing read");
    //     // self.with_dependent_mut(|_, reader| {
    //     //     let (reader, context) = std::mem::take(reader).0.context("no reader")?;
    //     //     WordBuffer::finish_read(reader, context).context("error finishing read")
    //     // })
    // }

    fn deserialize<T: DeserializeOwned>(&mut self) -> bitcode::Result<T> {
        self.with_dependent_mut(|_, reader| {
            let reader = reader
                .0
                .as_mut()
                .map_or_else(|| panic!("no reader"), |(reader, _)| reader);
            let with_gamma = OnlyGammaDecode::<T>::decode(Fixed, reader)?;
            Ok(with_gamma.0)
        })
    }

    fn decode<T: Decode>(&mut self, encoding: impl Encoding) -> bitcode::Result<T> {
        self.with_dependent_mut(|_, reader| {
            let reader = reader
                .0
                .as_mut()
                .map_or_else(|| panic!("no reader"), |(reader, _)| reader);
            T::decode(encoding, reader)
        })
    }

    fn start_read(bytes: &[u8]) -> Self {
        BitcodeReader::new(
            UnsafeCell::new(WordBuffer::with_capacity(bytes.len())),
            |buffer| {
                // safety: we just created the buffer and nothing else had access to it
                // we need to get a mutable reference to the buffer to take ownership of it
                unsafe {
                    let (reader, context) = buffer.deref_mut().start_read(bytes);
                    Reader(Some((reader, context)))
                }
            },
        )
    }

    fn finish_read(&mut self) -> bitcode::Result<()> {
        self.with_dependent_mut(|_, reader| {
            let Some((reader, context)) = std::mem::take(reader).0 else {
                panic!("no reader");
            };
            WordBuffer::finish_read(reader, context)
        })
    }
}

impl BitcodeReader {
    pub(crate) fn start_read_with_buffer(bytes: &[u8], buffer: WordBuffer) -> Self {
        BitcodeReader::new(UnsafeCell::new(buffer), |buffer| {
            // safety: we just created the buffer and nothing else had access to it
            // we need to get a mutable reference to the buffer to take ownership of it
            unsafe {
                let (reader, context) = buffer.deref_mut().start_read(bytes);
                Reader(Some((reader, context)))
            }
        })
    }
}
