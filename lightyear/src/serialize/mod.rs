//! Serialization and deserialization of types

use crate::serialize::reader::Reader;
use crate::serialize::varint::{varint_len, VarIntReadExt, VarIntWriteExt};
use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::Bytes;

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

impl<M: ToBytes> ToBytes for Option<M> {
    fn len(&self) -> usize {
        match self {
            Some(value) => 1 + value.len(),
            None => 1,
        }
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        match self {
            Some(value) => {
                buffer.write_u8(1)?;
                value.to_bytes(buffer)?;
            }
            None => {
                buffer.write_u8(0)?;
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let has_value = buffer.read_u8()? != 0;
        if has_value {
            Ok(Some(M::from_bytes(buffer)?))
        } else {
            Ok(None)
        }
    }
}

/// For Bytes, when we read instead of allocating we just create a new Bytes by slicing the buffer
impl ToBytes for Bytes {
    fn len(&self) -> usize {
        varint_len(self.len() as u64) + self.len()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_varint(self.len() as u64)?;
        buffer.write_all(self.as_ref())?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let len = buffer.read_varint()? as usize;
        let bytes = buffer.split_len(len);
        Ok(Bytes::from(bytes))
    }
}

macro_rules! impl_tuple_query_data {
    ($($name: ident),*) => {

        #[allow(non_snake_case)]
        #[allow(clippy::unused_unit)]
        // SAFETY: defers to soundness `$name: WorldQuery` impl
        impl<$($name: ToBytes),*> ToBytes for ($($name,)*) {
            fn len(&self) -> usize {
                let ($($name,)*) = self;
                let mut len = 0;
                $(len += $name.len();)*
                len
            }

            fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
                let ($($name,)*) = self;
                $($name.to_bytes(buffer)?;)*
                Ok(())
            }

            fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
                Ok(($($name::from_bytes(buffer)?,)*))
            }
        }
    };
}

bevy::utils::all_tuples!(impl_tuple_query_data, 1, 8, P);

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
