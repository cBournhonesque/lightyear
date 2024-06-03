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
            0: Cursor::new(value),
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
}
