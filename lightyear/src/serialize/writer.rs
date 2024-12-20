//! Writer that can reuse memory allocation
//!
//! See `<https://stackoverflow.com/questions/73725299/will-the-native-buffer-owned-by-bytesmut-keep-growing>`
//! for more details.
//!
//! The idea is that we have one allocation under the [`BytesMut`], when we finish writing a message,
//! we can split the message of as a separate [`Bytes`], but
use bytes::{BufMut, Bytes, BytesMut};
use std::io::Write;

#[derive(Debug)]
pub struct Writer(bytes::buf::Writer<BytesMut>);

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
        Self::with_capacity(10)
    }
}
impl Writer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(BytesMut::with_capacity(capacity).writer())
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
}
