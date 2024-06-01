//! Serialization and deserialization of types

use std::io::Seek;

pub mod bitcode;
pub mod reader;
pub(crate) mod varint;
pub mod writer;

pub type RawData = Vec<u8>;

#[derive(thiserror::Error, Debug)]
pub enum SerializationError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Invalid packet type")]
    InvalidPacketType,
    #[error("Substraction overflow")]
    SubstractionOverflow,
}

pub trait ToBytes {
    fn len(&self) -> usize;
    fn to_bytes<T: byteorder::WriteBytesExt>(
        &self,
        buffer: &mut T,
    ) -> Result<(), SerializationError>;

    fn from_bytes<T: byteorder::ReadBytesExt + Seek>(
        buffer: &mut T,
    ) -> Result<Self, SerializationError>
    where
        Self: Sized;
}
