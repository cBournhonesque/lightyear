#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bytes::Bytes;
#[cfg(not(feature = "std"))]
use no_std_io2::io::{Cursor, Read, Result, Seek, SeekFrom};
#[cfg(feature = "std")]
use std::io::{Cursor, Read, Result, Seek, SeekFrom};

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

    pub(crate) fn remaining(&self) -> usize {
        // bytes::Buf is only implemented for std::io::Cursor, not no_std
        // so we copy the implementation here
        let len = self.0.get_ref().as_ref().len();
        let pos = self.0.position();

        if pos >= len as u64 {
            return 0;
        }

        len - pos as usize
        // self.0.remaining()
    }
}
