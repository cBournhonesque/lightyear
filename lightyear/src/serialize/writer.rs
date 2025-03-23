//! Writer that can reuse memory allocation
//!
//! See `<https://stackoverflow.com/questions/73725299/will-the-native-buffer-owned-by-bytesmut-keep-growing>`
//! for more details.
//!
//! The idea is that we have one allocation under the [`BytesMut`], when we finish writing a message,
//! we can split the message of as a separate [`Bytes`], but

use no_std_io2::io::{Result, Write};
use bytes::{BufMut, Bytes, BytesMut};
use no_std_io2::io;
#[cfg(feature = "std")]
pub(crate) use std::Writer;
#[cfg(not(feature = "std"))]
pub(crate) use no_std::Writer;
use crate::serialize::SerializationError;
use crate::serialize::varint::varint_len;

#[cfg(feature = "std")]
pub(crate) mod std {
    use super::*;

    #[derive(Debug)]
    pub struct Writer(bytes::buf::Writer<BytesMut>);

    impl From<BytesMut> for Writer {
        fn from(value: BytesMut) -> Self {
            Self(value.writer())
        }
    }

    #[cfg(feature = "std")]
    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> Result<()> {
            self.0.flush()
        }
    }


    impl Writer {

        pub(crate) fn with_capacity(capacity: usize) -> Self {
            Self(BytesMut::with_capacity(capacity).writer())
        }

        pub(crate) fn len(&self) -> usize {
            self.0.get_ref().len()
        }

        pub(crate) fn position(&self) -> usize {
            self.len()
        }

        pub(crate) fn as_mut(&mut self) -> &mut [u8] {
            self.0.get_mut().as_mut()
        }

        // TODO: how do reduce capacity over time?
        /// Split the current bytes written as a separate [`Bytes`].
        ///
        /// Retains any additional capacity. O(1) operation.
        pub(crate) fn split(&mut self) -> Bytes {
            self.0.get_mut().split().freeze()
        }

        // TODO: normally there is no need to reset, because once all the messages that have been split
        //  are dropped, the writer will move the current data to the front of the buffer to reuse memory
        //  All the split bytes messages are dropped at Send for unreliable senders, but NOT for reliable
        //  senders, think about what to do for that! Maybe do a clone there to drop the message?
        /// Reset the writer but keeps the underlying allocation
        pub(crate) fn reset(&mut self) {
            self.0.get_mut().clear();
        }

        // by convention, to_* functions with non-Copy self types usually take a &self, but not here.
        /// Consume the writer to get the RawData
        #[allow(clippy::wrong_self_convention)]
        pub(crate) fn to_bytes(self) -> Bytes {
            self.0.into_inner().into()
        }

        // by convention, to_* functions with non-Copy self types usually take a &self, but not here.
        /// Consume the writer to get the RawData
        #[allow(clippy::wrong_self_convention)]
        pub(crate) fn to_bytes_mut(self) -> BytesMut {
            self.0.into_inner().into()
        }
    }

}
#[cfg(not(feature = "std"))]
pub(crate) mod no_std {
    use core::cmp;
    use bincode::error::EncodeError;
    use super::*;
    #[derive(Debug)]
    pub struct Writer(BytesMut);

    impl From<BytesMut> for Writer {
        fn from(value: BytesMut) -> Self {
            Self(value)
        }
    }

    impl Write for Writer {
        fn write(&mut self, src: &[u8]) -> Result<usize> {
            let n = cmp::min(self.0.remaining_mut(), src.len());

            self.0.put_slice(&src[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }

    impl Writer {

        pub(crate) fn with_capacity(capacity: usize) -> Self {
            Self(BytesMut::with_capacity(capacity))
        }

        pub(crate) fn len(&self) -> usize {
            self.0.as_ref().len()
        }

        pub(crate) fn position(&self) -> usize {
            self.len()
        }

        pub(crate) fn as_mut(&mut self) -> &mut [u8] {
            self.0.as_mut()
        }


        // TODO: how do reduce capacity over time?
        /// Split the current bytes written as a separate [`Bytes`].
        ///
        /// Retains any additional capacity. O(1) operation.
        pub(crate) fn split(&mut self) -> Bytes {
            self.0.split().freeze()
        }

        // TODO: normally there is no need to reset, because once all the messages that have been split
        //  are dropped, the writer will move the current data to the front of the buffer to reuse memory
        //  All the split bytes messages are dropped at Send for unreliable senders, but NOT for reliable
        //  senders, think about what to do for that! Maybe do a clone there to drop the message?
        /// Reset the writer but keeps the underlying allocation
        pub(crate) fn reset(&mut self) {
            self.0.clear();
        }

        // by convention, to_* functions with non-Copy self types usually take a &self, but not here.
        /// Consume the writer to get the RawData
        #[allow(clippy::wrong_self_convention)]
        pub(crate) fn to_bytes(self) -> Bytes {
            self.0.into()
        }

        #[allow(clippy::wrong_self_convention)]
        pub(crate) fn to_bytes_mut(self) -> BytesMut {
            self.0
        }
    }

    /// We need to provide our own implementation of bincode::enc::write::Writer.
    /// We cannot use the SliceWriter because it supposes that the slice is immutable
    impl bincode::enc::write::Writer for Writer {
        #[inline(always)]
        fn write(&mut self, bytes: &[u8]) -> core::result::Result<(), EncodeError> {
            self.write_all(bytes)
                .map_err(|inner| EncodeError::Other("encode error"))?;
            Ok(())
        }
    }
}

impl Default for Writer {
    fn default() -> Self {
        // TODO: we start with some capacity, benchmark how much we need
        Self::with_capacity(10)
    }
}


pub trait WriteInteger: Write {
    #[inline]
    fn write_u8(&mut self, n: u8) -> Result<()> {
        self.write_all(&[n])
    }

    #[inline]
    fn write_u16(&mut self, n: u16) -> Result<()> {
        let mut buf = [0; 2];
        buf[..2].copy_from_slice(&n.to_be_bytes());
        self.write_all(&buf)
    }
    #[inline]
    fn write_u32(&mut self, n: u32) -> Result<()> {
        let mut buf = [0; 4];
        buf[..4].copy_from_slice(&n.to_be_bytes());
        self.write_all(&buf)
    }
    #[inline]
    fn write_u64(&mut self, n: u64) -> Result<()> {
        let mut buf = [0; 8];
        buf[..8].copy_from_slice(&n.to_be_bytes());
        self.write_all(&buf)
    }

    #[inline]
    fn write_i8(&mut self, n: i8) -> Result<()> {
        self.write_u8(n as u8)
    }

    #[inline]
    fn write_i16(&mut self, n: i16) -> Result<()> {
        Self::write_u16(self, n as u16)
    }

    #[inline]
    fn write_i32(&mut self, n: i32) -> Result<()> {
        Self::write_u32(self, n as u32)
    }

    #[inline]
    fn write_i64(&mut self, n: i64) -> Result<()> {
        Self::write_u64(self, n as u64)
    }

    /// Write a variable length integer to the writer, in network byte order
    fn write_varint(&mut self, value: u64) -> core::result::Result<(), SerializationError> {
        let len = varint_len(value);
        match len {
            1 => self.write_u8(value as u8)?,
            2 => {
                let val = (value as u16) | 0x4000;
                self.write_u16(val)?;
            }
            4 => {
                let val = (value as u32) | 0x8000_0000;
                self.write_u32(val)?;
            }
            8 => {
                let val = value | 0xc000_0000_0000_0000;
                self.write_u64(val)?;
            }
            _ => return Err(io::Error::other("value is too large for varint").into()),
        };

        Ok(())
    }
}

impl<T: Write> WriteInteger for T {}