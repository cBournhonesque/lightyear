use crate::serialize::RawData;
use bytes::Bytes;
use std::io::{Cursor, Seek, SeekFrom};

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
        self.seek(pos)
    }
}

impl Reader {
    /// Returns the underlying RawData
    pub(crate) fn consume(self) -> RawData {
        self.0.into_inner()
    }
}
