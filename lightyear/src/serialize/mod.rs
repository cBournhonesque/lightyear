//! Serialization and deserialization of types

use crate::serialize::reader::Reader;
use crate::serialize::varint::varint_len;
use byteorder::{ReadBytesExt, WriteBytesExt};

pub mod reader;
pub(crate) mod varint;
pub mod writer;

pub type RawData = Vec<u8>;

#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum SerializationError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Invalid packet type")]
    InvalidPacketType,
    #[error("Invalid value")]
    InvalidValue,
    #[error("Substraction overflow")]
    SubstractionOverflow,
    #[error(transparent)]
    BincodeEncode(#[from] bincode::error::EncodeError),
    #[error(transparent)]
    BincodeDecode(#[from] bincode::error::DecodeError),
}

#[allow(clippy::len_without_is_empty)]
pub trait ToBytes {
    fn len(&self) -> usize;
    fn to_bytes<T: byteorder::WriteBytesExt>(
        &self,
        buffer: &mut T,
    ) -> Result<(), SerializationError>;

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized;
}

impl<M: ToBytes> ToBytes for Vec<M> {
    fn len(&self) -> usize {
        varint_len(self.len() as u64) + self.iter().map(ToBytes::len).sum::<usize>()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_u64::<byteorder::NetworkEndian>(self.len() as u64)?;
        self.iter().try_for_each(|item| item.to_bytes(buffer))?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let len = buffer.read_u64::<byteorder::NetworkEndian>()? as usize;
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(M::from_bytes(buffer)?);
        }
        Ok(vec)
    }
}
