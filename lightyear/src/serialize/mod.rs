//! Serialization and deserialization of types

use crate::serialize::reader::Reader;
use crate::serialize::varint::{varint_len, VarIntReadExt, VarIntWriteExt};
use bevy::platform_support::collections::HashMap;
use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::Bytes;
use core::hash::{BuildHasher, Hash};
#[cfg(feature = "std")]
use std::io;
#[cfg(not(feature = "std"))]
use {
    alloc::{vec, vec::Vec},
    no_std_io2::{io, io::Write},
};

pub mod reader;
pub(crate) mod varint;
pub mod writer;

pub type RawData = Vec<u8>;

#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum SerializationError {
    #[error(transparent)]
    Io(#[from] io::Error),
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
    fn to_bytes<T: WriteBytesExt>(
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

/// When we read, instead of allocating we just create a new Bytes by slicing the buffer
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
        Ok(bytes)
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

variadics_please::all_tuples!(impl_tuple_query_data, 1, 8, P);

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
        // TODO: if we know the MIN_LEN we can preallocate
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(M::from_bytes(buffer)?);
        }
        Ok(vec)
    }
}

impl<K: ToBytes + Eq + Hash, V: ToBytes, S: Default + BuildHasher> ToBytes for HashMap<K, V, S> {
    fn len(&self) -> usize {
        varint_len(self.len() as u64) + self.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_u64::<byteorder::NetworkEndian>(self.len() as u64)?;
        self.iter().try_for_each(|(k, v)| {
            k.to_bytes(buffer)?;
            v.to_bytes(buffer)?;
            Ok::<(), SerializationError>(())
        })?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let len = buffer.read_u64::<byteorder::NetworkEndian>()? as usize;
        // TODO: if we know the MIN_LEN we can preallocate
        let mut res = HashMap::with_capacity_and_hasher(len, S::default());
        for _ in 0..len {
            let key = K::from_bytes(buffer)?;
            let value = V::from_bytes(buffer)?;
            res.insert(key, value);
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;

    #[test]
    fn test_serialize_bytes() {
        let a: Bytes = vec![7; 100].into();
        let mut writer = Writer::with_capacity(5);
        a.to_bytes(&mut writer).unwrap();

        let mut reader = Reader::from(writer.to_bytes());
        let read = Bytes::from_bytes(&mut reader).unwrap();
        assert_eq!(a, read);
    }
}
