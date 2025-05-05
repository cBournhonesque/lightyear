//! Serialization and deserialization of types
use crate::serialize::reader::{ReadInteger, ReadVarInt, Reader};
use crate::serialize::varint::{varint_len};
use bevy::platform::collections::HashMap;
use bytes::Bytes;
use core::hash::{BuildHasher, Hash};
use no_std_io2::io;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

pub mod reader;
pub(crate) mod varint;
pub mod writer;

use crate::serialize::writer::WriteInteger;

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
    fn bytes_len(&self) -> usize;
    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> Result<(), SerializationError>;

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized;
}

impl<M: ToBytes> ToBytes for Option<M> {
    fn bytes_len(&self) -> usize {
        match self {
            Some(value) => 1 + value.bytes_len(),
            None => 1,
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
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
    fn bytes_len(&self) -> usize {
        varint_len(self.len() as u64) + self.len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
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

impl ToBytes for u8 {
    fn bytes_len(&self) -> usize {
        1
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u8(*self)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(buffer.read_u8()?)
    }
}

macro_rules! impl_tuple_query_data {
    ($($name: ident),*) => {

        #[allow(non_snake_case)]
        #[allow(clippy::unused_unit)]
        // SAFETY: defers to soundness `$name: WorldQuery` impl
        impl<$($name: ToBytes),*> ToBytes for ($($name,)*) {
            fn bytes_len(&self) -> usize {
                let ($($name,)*) = self;
                let mut len = 0;
                $(len += $name.bytes_len();)*
                len
            }

            fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
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
    fn bytes_len(&self) -> usize {
        varint_len(self.len() as u64) + self.iter().map(ToBytes::bytes_len).sum::<usize>()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_varint(self.len() as u64)?;
        self.iter().try_for_each(|item| item.to_bytes(buffer))?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let len = buffer.read_varint()? as usize;
        // TODO: if we know the MIN_LEN we can preallocate
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(M::from_bytes(buffer)?);
        }
        Ok(vec)
    }
}

impl<K: ToBytes + Eq + Hash, V: ToBytes, S: Default + BuildHasher> ToBytes for HashMap<K, V, S> {
    fn bytes_len(&self) -> usize {
        varint_len(self.len() as u64) + self.iter().map(|(k, v)| k.bytes_len() + v.bytes_len()).sum::<usize>()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_varint(self.len() as u64)?;
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
        let len = buffer.read_varint()? as usize;
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
    #[cfg(not(feature = "std"))]
    use alloc::vec;
    use bevy::prelude::Entity;

    #[test]
    fn test_serialize_bytes() {
        let a: Bytes = vec![7; 100].into();
        let mut writer = Writer::with_capacity(5);
        a.to_bytes(&mut writer).unwrap();

        let mut reader = Reader::from(writer.to_bytes());
        let read = Bytes::from_bytes(&mut reader).unwrap();
        assert_eq!(a, read);
    }

    #[test]
    fn test_serialize_vec() {
        let a  = [5; 10].to_vec();
        let mut writer = Writer::with_capacity(5);
        a.to_bytes(&mut writer).unwrap();
        let b: Vec<u8>  = vec![];
        b.to_bytes(&mut writer).unwrap();


        let mut reader = Reader::from(writer.to_bytes());
        let a_read = Vec::<u8>::from_bytes(&mut reader).unwrap();
        let b_read = Vec::<u8>::from_bytes(&mut reader).unwrap();
        assert_eq!(a, a_read);
        assert_eq!(b, b_read);
    }

    #[test]
    fn test_serialize_map() {
        let mut a  = HashMap::default();
        a.insert(1, 2);
        a.insert(3, 4);
        let mut writer = Writer::with_capacity(5);
        a.to_bytes(&mut writer).unwrap();

        let mut reader = Reader::from(writer.to_bytes());
        let read = HashMap::<u8, u8>::from_bytes(&mut reader).unwrap();
        assert_eq!(a, read);
    }

    #[test]
    fn test_serialize_entity() {
        let a  = Entity::from_raw(0);
        let b  = Entity::from_raw(23);
        let c  = Entity::from_raw(u32::MAX);
        let mut writer = Writer::with_capacity(5);
        a.to_bytes(&mut writer).unwrap();
        b.to_bytes(&mut writer).unwrap();
        c.to_bytes(&mut writer).unwrap();

        let mut reader = Reader::from(writer.to_bytes());
        let read_a = Entity::from_bytes(&mut reader).unwrap();
        let read_b = Entity::from_bytes(&mut reader).unwrap();
        let read_c = Entity::from_bytes(&mut reader).unwrap();
        assert_eq!(a, read_a);
        assert_eq!(b, read_b);
        assert_eq!(c, read_c);
    }
}
