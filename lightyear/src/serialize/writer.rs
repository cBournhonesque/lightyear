use crate::serialize::RawData;
use std::io::{Cursor, Write};

pub struct Writer(Cursor<RawData>);

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Default for Writer {
    fn default() -> Self {
        // TODO: we start with some capacity, benchmark how much we need
        Self::with_capacity(20)
    }
}
impl Writer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(Cursor::new(Vec::with_capacity(capacity)))
    }

    /// Reset the writer but keeps the underlying allocation
    pub(crate) fn reset(&mut self) {
        self.0.set_position(0);
    }

    /// Consume the writer to get the RawData
    pub(crate) fn consume(self) -> RawData {
        self.0.into_inner()
    }
}
