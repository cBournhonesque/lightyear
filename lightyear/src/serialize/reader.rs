use no_std_io2::io::{Cursor, Read, Result, Seek, SeekFrom, Error};
use bytes::Bytes;
#[cfg(not(feature = "std"))]
pub(crate) use no_std::Reader;
#[cfg(feature = "std")]
pub(crate) use std::Reader;
use crate::serialize::SerializationError;
use crate::serialize::varint::varint_parse_len;

#[cfg(feature = "std")]
pub(crate) mod std {
    use bytes::Buf;
    use super::*;

    #[derive(Clone)]
    pub struct Reader(Cursor<Bytes>);


    impl From<Bytes> for Reader {
        fn from(value: Bytes) -> Self {
            // TODO: check that this has no cost
            Self(Cursor::new(value))
        }
    }

    impl From<Vec<u8>> for Reader {
        fn from(value: Vec<u8>) -> Self {
            Self(Cursor::new(value.into()))
        }
    }

    impl Seek for Reader {
        fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
            self.0.seek(pos)
        }
    }

    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.0.read(buf)
        }
    }

    impl Reader {
        /// Returns the underlying RawData
        pub(crate) fn consume(self) -> Bytes {
            self.0.into_inner()
        }

        pub(crate) fn as_ref(&self) -> &[u8] {
            self.0.get_ref().as_ref()
        }

        pub(crate) fn len(&self) -> usize {
            self.0.get_ref().len()
        }

        /// Split of the next `len` bytes from the reader into a separate Bytes.
        ///
        /// This doesn't allocate and just increases some reference counts. O(1) cost.
        pub(crate) fn split_len(&mut self, len: usize) -> Bytes {
            let current_pos = self.0.position() as usize;
            let new_pos = current_pos + len;
            // slice off the subset into a separate Bytes
            let bytes = self.0.get_ref().slice(current_pos..new_pos);
            // increment the position
            self.0.set_position(new_pos as u64);
            bytes
        }

        pub(crate) fn has_remaining(&self) -> bool {
            self.remaining() > 0
        }

        pub(crate) fn position(&self) -> u64 {
            self.0.position()
        }

        pub(crate) fn set_position(&mut self, pos: u64) {
            self.0.set_position(pos)
        }

        pub(crate) fn remaining(&self) -> usize {
            self.0.remaining()
        }
    }
}

#[cfg(not(feature = "std"))]
pub(crate) mod no_std {
    use super::*;
    use alloc::vec::Vec;
    use bincode::error::DecodeError;

    #[derive(Clone)]
    pub struct Reader(Cursor<Bytes>);

    #[inline(always)]
    fn saturating_sub_usize_u64(a: usize, b: u64) -> usize {
        use core::convert::TryFrom;
        match usize::try_from(b) {
            Ok(b) => a.saturating_sub(b),
            Err(_) => 0,
        }
    }

    impl From<Bytes> for Reader {
        fn from(value: Bytes) -> Self {
            // TODO: check that this has no cost
            Self(Cursor::new(value))
        }
    }

    impl From<Vec<u8>> for Reader {
        fn from(value: Vec<u8>) -> Self {
            Self(Cursor::new(value.into()))
        }
    }

    impl Seek for Reader {
        fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
            self.0.seek(pos)
        }
    }

    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.0.read(buf)
        }
    }

    impl Reader {
        /// Returns the underlying RawData
        pub(crate) fn consume(self) -> Bytes {
            self.0.into_inner()
        }

        pub(crate) fn as_ref(&self) -> &[u8] {
            self.0.get_ref().as_ref()
        }

        pub(crate) fn len(&self) -> usize {
            self.0.get_ref().len()
        }

        /// Split of the next `len` bytes from the reader into a separate Bytes.
        ///
        /// This doesn't allocate and just increases some reference counts. O(1) cost.
        pub(crate) fn split_len(&mut self, len: usize) -> Bytes {
            let current_pos = self.0.position() as usize;
            let new_pos = current_pos + len;
            // slice off the subset into a separate Bytes
            let bytes = self.0.get_ref().slice(current_pos..new_pos);
            // increment the position
            self.0.set_position(new_pos as u64);
            bytes
        }

        pub(crate) fn has_remaining(&self) -> bool {
            self.remaining() > 0
        }

        pub(crate) fn position(&self) -> u64 {
            self.0.position()
        }

        pub(crate) fn set_position(&mut self, pos: u64) {
            self.0.set_position(pos)
        }

        pub(crate) fn remaining(&self) -> usize {
            // copied from the Buf implementation for std::io::Cursor in tokio::bytes
            saturating_sub_usize_u64(self.len(), self.position())
        }
    }

    /// We cannot decode into a Slice because the slice is not Extendable
    impl bincode::de::read::Reader for Reader {
        #[inline(always)]
        fn read(&mut self, bytes: &mut [u8]) -> core::result::Result<(), DecodeError> {
            self.read_exact(bytes)
                .map_err(|inner| DecodeError::Other("could not decode"))
        }
    }
}


pub trait ReadInteger: Read {
    #[inline]
    fn read_u8(&mut self) -> Result<u8> {
        let mut buf = [0; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    #[inline]
    fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf[..2].try_into().unwrap()))
    }

    #[inline]
    fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf[..4].try_into().unwrap()))
    }

    #[inline]
    fn read_u64(&mut self) -> Result<u64> {
        let mut buf = [0; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_be_bytes(buf[..8].try_into().unwrap()))
    }

    #[inline]
    fn read_i8(&mut self) -> Result<i8> {
        let mut buf = [0; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0] as i8)
    }

    #[inline]
    fn read_i16(&mut self) -> Result<i16> {
        let mut buf = [0; 2];
        self.read_exact(&mut buf)?;
        Ok(i16::from_be_bytes(buf[..2].try_into().unwrap()))
    }

    #[inline]
    fn read_i32(&mut self) -> Result<i32> {
        let mut buf = [0; 4];
        self.read_exact(&mut buf)?;
        Ok(i32::from_be_bytes(buf[..4].try_into().unwrap()))
    }

    #[inline]
    fn read_i64(&mut self) -> Result<i64> {
        let mut buf = [0; 8];
        self.read_exact(&mut buf)?;
        Ok(i64::from_be_bytes(buf[..8].try_into().unwrap()))
    }
}

pub trait ReadVarInt: ReadInteger + Seek {
    /// Reads an unsigned variable-length integer in network byte-order from
    /// the current offset and advances the buffer.
    fn read_varint(&mut self) -> core::result::Result<u64, SerializationError> {
        let first = self.read_u8()?;
        let len = varint_parse_len(first);
        let out = match len {
            1 => u64::from(first),
            2 => {
                // TODO: we actually don't need seek, no? we can just read the next few bytes ...
                // go back 1 byte because we read the first byte above
                self.seek(SeekFrom::Current(-1))?;
                u64::from(self.read_u16()? & 0x3fff)
            }
            4 => {
                self.seek(SeekFrom::Current(-1))?;
                u64::from(self.read_u32()? & 0x3fffffff)
            }
            8 => {
                self.seek(SeekFrom::Current(-1))?;
                self.read_u64()? & 0x3fffffffffffffff
            }
            _ => return Err(Error::other("value is too large for varint").into()),
        };
        Ok(out)
    }
}

impl<T: Read>  ReadInteger for T {}
impl<T: Read + Seek>  ReadVarInt for T {}


#[cfg(test)]
mod tests {
    use no_std_io2::io;
    use super::*;
    use crate::serialize::writer::WriteInteger;

    #[cfg(not(feature = "std"))]
    use alloc::vec;

    #[test]
    fn test_read_integer() {
        let mut writer = vec![];
        writer.write_u8(1).unwrap();
        writer.write_u16(2).unwrap();
        writer.write_u32(3).unwrap();
        writer.write_u64(4).unwrap();
        writer.write_i8(-1).unwrap();
        writer.write_i16(-2).unwrap();
        writer.write_i32(-3).unwrap();
        writer.write_i64(-4).unwrap();

        let mut reader = io::Cursor::new(writer);

        assert_eq!(reader.read_u8().unwrap(), 1);
        assert_eq!(reader.read_u16().unwrap(), 2);
        assert_eq!(reader.read_u32().unwrap(), 3);
        assert_eq!(reader.read_u64().unwrap(), 4);
        assert_eq!(reader.read_i8().unwrap(), -1);
        assert_eq!(reader.read_i16().unwrap(), -2);
        assert_eq!(reader.read_i32().unwrap(), -3);
        assert_eq!(reader.read_i64().unwrap(), -4);
    }

}