use crate::serialize::RawData;
use bytes::Bytes;
use std::io::{Cursor, Read, Seek, SeekFrom};

pub struct Reader(Cursor<RawData>);

impl From<Bytes> for Reader {
    fn from(value: Bytes) -> Self {
        // TODO: check that this has no cost
        Self {
            0: Cursor::new(value.into()),
        }
    }
}

impl From<Vec<u8>> for Reader {
    fn from(value: Vec<u8>) -> Self {
        Self {
            0: Cursor::new(value.into()),
        }
    }
}

impl Seek for Reader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Reader {
    /// Returns the underlying RawData
    pub(crate) fn consume(self) -> RawData {
        self.0.into_inner()
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
}
