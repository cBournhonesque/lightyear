use crate::read::Read;
use crate::word_buffer::WordBuffer;
use crate::write::Write;
use crate::{Result, E};

/// A buffer for reusing allocations between any number of calls to [`Buffer::encode`] and/or
/// [`Buffer::decode`].
///
/// ### Usage
/// ```edition2021
/// use bitcode::Buffer;
///
/// // We preallocate buffers with capacity 1000. This will allow us to encode and decode without
/// // any allocations as long as the encoded object takes less than 1000 bytes.
/// let bytes = 1000;
/// let mut encode_buf = Buffer::with_capacity(bytes);
/// let mut decode_buf = Buffer::with_capacity(bytes);
///
/// // The object that we will encode.
/// let target: [u8; 5] = [1, 2, 3, 4, 5];
///
/// // We encode into `encode_buf`. This won't cause any allocations.
/// let encoded: &[u8] = encode_buf.encode(&target).unwrap();
/// assert!(encoded.len() <= bytes, "oh no we allocated");
///
/// // We decode into `decode_buf` because `encoded` is borrowing `encode_buf`.
/// let decoded: [u8; 5] = decode_buf.decode(&encoded).unwrap();
/// assert_eq!(target, decoded);
///
/// // If we need ownership of `encoded`, we can convert it to a vec.
/// // This will allocate, but it's still more efficient than calling bitcode::encode.
/// let _owned: Vec<u8> = encoded.to_vec();
/// ```
#[derive(Default)]
pub struct Buffer(pub WordBuffer);

impl Buffer {
    /// Constructs a new buffer without any capacity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs a new buffer with at least the specified capacity in bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(BufferTrait::with_capacity(capacity))
    }

    /// Returns the capacity in bytes.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

pub trait BufferTrait: Default {
    type Writer: Write;
    type Reader<'a>: Read;
    type Context;

    fn capacity(&self) -> usize;
    fn with_capacity(capacity: usize) -> Self;

    /// Clears the buffer.
    fn start_write(&mut self) -> Self::Writer;
    /// Returns the written bytes.
    fn finish_write(&mut self, writer: Self::Writer) -> &[u8];

    fn start_read<'a, 'b>(&'a mut self, bytes: &'b [u8]) -> (Self::Reader<'a>, Self::Context)
    where
        'a: 'b;
    /// Check for errors such as Eof and ExpectedEof
    fn finish_read(reader: Self::Reader<'_>, context: Self::Context) -> Result<()>;
    /// Overrides decoding errors with Eof since the reader might allow reading slightly past the
    /// end. Only WordBuffer currently does this.
    fn finish_read_with_result<T>(
        reader: Self::Reader<'_>,
        context: Self::Context,
        decode_result: Result<T>,
    ) -> Result<T> {
        let finish_result = Self::finish_read(reader, context);
        if let Err(e) = &finish_result {
            if e.same(&E::Eof.e()) {
                return Err(E::Eof.e());
            }
        }
        let t = decode_result?;
        finish_result?;
        Ok(t)
    }
}

#[cfg(all(test, not(miri), debug_assertions))]
mod tests {
    use crate::bit_buffer::BitBuffer;
    use crate::buffer::BufferTrait;
    use crate::word_buffer::WordBuffer;
    use paste::paste;

    macro_rules! test_with_capacity {
        ($name:ty, $t:ty) => {
            paste! {
                #[test]
                fn [<test_ $name _with_capacity>]() {
                    for cap in 0..200 {
                        let buf = $t::with_capacity(cap);
                        assert!(buf.capacity() >= cap, "with_capacity: {cap}, capacity {}", buf.capacity());
                    }
                }
            }
        }
    }

    test_with_capacity!(bit_buffer, BitBuffer);
    test_with_capacity!(word_buffer, WordBuffer);
}
