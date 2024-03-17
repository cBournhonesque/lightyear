use std::num::NonZeroUsize;

use anyhow::Result;
use bitcode::encoding::Encoding;
use bitcode::word::Word;
use bitcode::Decode;
use serde::de::DeserializeOwned;

pub trait ReadBuffer {
    /// Deserialize from the buffer using serde
    fn deserialize<T: DeserializeOwned>(&mut self) -> Result<T>;

    /// Deserialize from the buffer using bitcode
    fn decode<T: Decode>(&mut self, encoding: impl Encoding) -> Result<T>;

    /// Copy the bytes into the buffer, so that we can deserialize them
    fn start_read(bytes: &[u8]) -> Self;

    /// Check for errors such as Eof and ExpectedEof
    fn finish_read(&mut self) -> Result<()>;
}
