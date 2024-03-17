use crate::buffer::BufferTrait;
use crate::nightly::div_ceil;
use crate::read::Read;
use crate::word::*;
use crate::write::Write;
use crate::{Result, E};
use from_bytes_or_zeroed::FromBytesOrZeroed;
use std::array;
use std::num::NonZeroUsize;

/// A fast `Buffer` that operates on [`Word`]s.
#[derive(Debug, Default)]

pub struct WordBuffer {
    allocation: Allocation,
    read_bytes_buf: Box<[Word]>,
}

#[derive(Debug, Default)]
struct Allocation {
    allocation: Vec<Word>,
    written_words: usize,
}

impl Allocation {
    fn as_mut_slice(&mut self) -> &mut [Word] {
        self.allocation.as_mut_slice()
    }

    fn take_box(&mut self) -> Box<[Word]> {
        let vec = std::mem::take(&mut self.allocation);
        let mut box_ = if vec.capacity() == vec.len() {
            vec
        } else {
            // Must have been created by start_read. We need len and capacity to be equal to make
            // into_boxed_slice zero cost. If we zeroed up to capacity we could have a situation
            // where reading/writing to same buffer causes the whole capacity to be zeroed each
            // write (even if only a small portion of the buffer is used).
            vec![]
        }
        .into_boxed_slice();

        // Zero all the words that we could have written to.
        let written_words = self.written_words.min(box_.len());
        box_[0..written_words].fill(0);
        self.written_words = 0;
        debug_assert!(box_.iter().all(|&w| w == 0));

        box_
    }

    fn replace_box(&mut self, box_: Box<[Word]>, written_words: usize) {
        self.allocation = box_.into();
        self.written_words = written_words;
    }

    fn make_vec(&mut self) -> &mut Vec<Word> {
        self.written_words = usize::MAX;
        &mut self.allocation
    }
}

pub struct WordContext {
    input_bytes: usize,
}

impl WordBuffer {
    /// Extra [`Word`]s appended to the end of the input to make deserialization faster.
    /// 1 for peek_reserved_bits and another for read_zeros (which calls peek_reserved_bits).
    const READ_PADDING: usize = 2;
}

impl BufferTrait for WordBuffer {
    type Writer = WordWriter;
    type Reader<'a> = WordReader<'a>;
    type Context = WordContext;

    fn capacity(&self) -> usize {
        // Subtract the padding of 1 (added by alloc_index_plus_one).
        self.allocation.allocation.capacity().saturating_sub(1) * WORD_BYTES
    }

    fn with_capacity(cap: usize) -> Self {
        let mut me = Self::default();
        if cap == 0 {
            return me;
        }
        let mut writer = Self::Writer::default();

        // Convert len to index by subtracting 1.
        Self::Writer::alloc_index_plus_one(&mut writer.words, div_ceil(cap, WORD_BYTES) - 1);
        me.allocation.replace_box(writer.words, 0);
        me
    }

    fn start_write(&mut self) -> Self::Writer {
        let words = self.allocation.take_box();
        Self::Writer { words, index: 0 }
    }

    fn finish_write(&mut self, mut writer: Self::Writer) -> &[u8] {
        // write_zeros doesn't allocate, but it moves index so we allocate up to index at the end.
        let index = writer.index / WORD_BITS;
        if index >= writer.words.len() {
            // TODO could allocate exact amount instead of regular growth strategy.
            Self::Writer::alloc_index_plus_one(&mut writer.words, index);
        }

        let Self::Writer { words, index } = writer;
        let written_words = div_ceil(index, WORD_BITS);

        self.allocation.replace_box(words, written_words);
        let written_words = &mut self.allocation.as_mut_slice()[..written_words];

        // Swap bytes in each word (that was written to) if big endian.
        if cfg!(target_endian = "big") {
            written_words.iter_mut().for_each(|w| *w = w.swap_bytes());
        }

        let written_bytes = div_ceil(index, u8::BITS as usize);
        &bytemuck::cast_slice(written_words)[..written_bytes]
    }

    fn start_read<'a, 'b>(&'a mut self, bytes: &'b [u8]) -> (Self::Reader<'a>, Self::Context)
    where
        'a: 'b,
    {
        let words = self.allocation.make_vec();
        words.clear();

        // u8s rounded up to u64s plus 1 u64 padding.
        let capacity = div_ceil(bytes.len(), WORD_BYTES) + Self::READ_PADDING;
        words.reserve_exact(capacity);

        // Fast hot loop (would be nicer with array_chunks, but that requires nightly).
        let chunks = bytes.chunks_exact(WORD_BYTES);
        let remainder = chunks.remainder();
        words.extend(chunks.map(|chunk| {
            let chunk: &[u8; 8] = chunk.try_into().unwrap();
            Word::from_le_bytes(*chunk)
        }));

        // Remaining bytes.
        if !remainder.is_empty() {
            words.push(u64::from_le_bytes(array::from_fn(|i| {
                remainder.get(i).copied().unwrap_or_default()
            })));
        }

        // Padding so peek_reserved_bits doesn't ever go out of bounds.
        words.extend([0; Self::READ_PADDING]);
        debug_assert_eq!(words.len(), capacity);

        let reader = WordReader {
            inner: WordReaderInner { words, index: 0 },
            read_bytes_buf: &mut self.read_bytes_buf,
        };
        let context = WordContext {
            input_bytes: bytes.len(),
        };
        (reader, context)
    }

    fn finish_read(reader: Self::Reader<'_>, context: Self::Context) -> Result<()> {
        let read = reader.inner.index;
        let bytes_read = div_ceil(read, u8::BITS as usize);
        let index = read / WORD_BITS;
        let bits_written = read % WORD_BITS;

        if bits_written != 0 && reader.inner.words[index] & !((1 << bits_written) - 1) != 0 {
            return Err(E::ExpectedEof.e());
        }

        use std::cmp::Ordering::*;
        match bytes_read.cmp(&context.input_bytes) {
            Less => Err(E::ExpectedEof.e()),
            Equal => Ok(()),
            Greater => {
                // It is possible that we read more bytes than we have (bytes are rounded up to words).
                // We don't check this while deserializing to avoid degrading performance.
                Err(E::Eof.e())
            }
        }
    }
}

#[derive(Default)]
pub struct WordWriter {
    words: Box<[Word]>,
    index: usize,
}

impl WordWriter {
    /// Allocates at least `words` of zeroed memory.
    fn alloc(words: &mut Box<[Word]>, len: usize) {
        let new_cap = len.next_power_of_two().max(16);

        // TODO find a way to use Allocator::grow_zeroed safely (new bytemuck api?).
        let new = bytemuck::allocation::zeroed_slice_box(new_cap);

        let previous = std::mem::replace(words, new);
        words[..previous.len()].copy_from_slice(&previous);
    }

    // Allocates up to an `index + 1` in words if a bounds check fails.
    // Returns a mutable array of [index, index + 1] to avoid bounds checks near hot code.
    #[cold]
    fn alloc_index_plus_one(words: &mut Box<[Word]>, index: usize) -> &mut [Word; 2] {
        let end = index + 2;
        Self::alloc(words, end);
        (&mut words[index..end]).try_into().unwrap()
    }

    /// Ensures that space for `bytes` is allocated.\
    #[inline(always)]
    fn reserve_write_bytes(&mut self, bytes: usize) {
        let index = self.index / WORD_BITS + bytes / WORD_BYTES + 1;
        if index >= self.words.len() {
            Self::alloc_index_plus_one(&mut self.words, index);
        }
    }

    #[inline(always)]
    fn write_bits_inner(
        &mut self,
        word: Word,
        bits: usize,
        out_of_bounds: fn(&mut Box<[Word]>, usize) -> &mut [Word; 2],
    ) {
        debug_assert!(bits <= WORD_BITS);
        if bits != WORD_BITS {
            debug_assert_eq!(word, word & ((1 << bits) - 1));
        }

        let bit_index = self.index;
        self.index += bits;

        let index = bit_index / WORD_BITS;
        let bit_remainder = bit_index % WORD_BITS;

        // Only requires 1 branch in hot path.
        let slice = if let Some(w) = self.words.get_mut(index..index + 2) {
            w.try_into().unwrap()
        } else {
            out_of_bounds(&mut self.words, index)
        };
        slice[0] |= word << bit_remainder;
        slice[1] = (word >> 1) >> (WORD_BITS - bit_remainder - 1);
    }

    #[inline(always)]
    fn write_reserved_bits(&mut self, word: Word, bits: usize) {
        self.write_bits_inner(word, bits, |_, _| unreachable!());
    }

    fn write_reserved_words(&mut self, src: &[Word]) {
        debug_assert!(!src.is_empty());

        let bit_start = self.index;
        let bit_end = self.index + src.len() * WORD_BITS;
        self.index = bit_end;

        let start = bit_start / WORD_BITS;
        let end = div_ceil(bit_end, WORD_BITS);

        let shl = bit_start % WORD_BITS;
        let shr = WORD_BITS - shl;

        if shl == 0 {
            self.words[start..end].copy_from_slice(src)
        } else {
            let after_start = start + 1;
            let before_end = end - 1;

            let dst = &mut self.words[after_start..before_end];

            // Do bounds check outside loop. Makes compiler go brrr
            assert!(dst.len() < src.len());

            for (i, w) in dst.iter_mut().enumerate() {
                let a = src[i];
                let b = src[i + 1];
                debug_assert_eq!(*w, 0);
                *w = (a >> shr) | (b << shl)
            }

            self.words[start] |= src[0] << shl;
            debug_assert_eq!(self.words[before_end], 0);
            self.words[before_end] = *src.last().unwrap() >> shr
        }
    }
}

impl Write for WordWriter {
    #[inline(always)]
    fn write_bit(&mut self, v: bool) {
        let bit_index = self.index;
        self.index += 1;

        let index = bit_index / WORD_BITS;
        let bit_remainder = bit_index % WORD_BITS;

        *if let Some(w) = self.words.get_mut(index) {
            w
        } else {
            &mut Self::alloc_index_plus_one(&mut self.words, index)[0]
        } |= (v as Word) << bit_remainder;
    }

    #[inline(always)]
    fn write_bits(&mut self, word: Word, bits: usize) {
        self.write_bits_inner(word, bits, Self::alloc_index_plus_one);
    }

    #[inline(always)]
    fn write_bytes(&mut self, bytes: &[u8]) {
        #[inline(always)]
        fn write_0_to_8_bytes(me: &mut WordWriter, bytes: &[u8]) {
            debug_assert!(bytes.len() <= 8);
            me.write_reserved_bits(
                u64::from_le_bytes_or_zeroed(bytes),
                bytes.len() * u8::BITS as usize,
            );
        }

        // Slower for small inputs. Doesn't work on big endian since it bytemucks u64 to bytes.
        #[inline(never)]
        fn write_many_bytes(me: &mut WordWriter, bytes: &[u8]) {
            assert!(!cfg!(target_endian = "big"));

            // TODO look into align_to specification to see if any special cases are required.
            let (a, b, c) = bytemuck::pod_align_to::<u8, Word>(bytes);
            write_0_to_8_bytes(me, a);
            me.write_reserved_words(b);
            write_0_to_8_bytes(me, c);
        }

        if bytes.is_empty() {
            return;
        }

        self.reserve_write_bytes(bytes.len());

        // Fast case for short bytes. Both methods are about the same speed at 75 bytes.
        // write_many_bytes doesn't work on big endian.
        if bytes.len() < 75 || cfg!(target_endian = "big") {
            let mut bytes = bytes;
            while bytes.len() > 8 {
                let b8: &[u8; 8] = bytes[0..8].try_into().unwrap();
                self.write_reserved_bits(Word::from_le_bytes(*b8), WORD_BITS);
                bytes = &bytes[8..]
            }
            write_0_to_8_bytes(self, bytes);
        } else {
            write_many_bytes(self, bytes)
        }
    }

    #[inline(always)]
    fn write_zeros(&mut self, bits: usize) {
        debug_assert!(bits <= WORD_BITS);
        self.index += bits;
    }

    fn num_bits_written(&self) -> usize {
        self.index
    }
}

struct WordReaderInner<'a> {
    words: &'a [Word],
    index: usize,
}

impl WordReaderInner<'_> {
    #[inline(always)]
    fn peek_reserved_bits(&self, bits: usize) -> Word {
        debug_assert!((1..=WORD_BITS).contains(&bits));
        let bit_index = self.index;

        let index = bit_index / WORD_BITS;
        let bit_remainder = bit_index % WORD_BITS;

        let a = self.words[index] >> bit_remainder;
        let b = (self.words[index + 1] << 1) << (WORD_BITS - 1 - bit_remainder);

        // Clear bits at end (don't need to do in ser because bits at end are zeroed).
        let extra_bits = WORD_BITS - bits;
        ((a | b) << extra_bits) >> extra_bits
    }

    #[inline(always)]
    fn read_reserved_bits(&mut self, bits: usize) -> Word {
        let v = self.peek_reserved_bits(bits);
        self.index += bits;
        v
    }

    /// Faster [`Read::reserve_bits`] that can elide bounds checks for `bits` in range `1..=64`.
    #[inline(always)]
    fn reserve_1_to_64_bits(&self, bits: usize) -> Result<()> {
        debug_assert!((1..=WORD_BITS).contains(&bits));

        let read = self.index / WORD_BITS;
        let len = self.words.len();
        if read + 1 >= len {
            // TODO hint as unlikely.
            Err(E::Eof.e())
        } else {
            Ok(())
        }
    }
}

pub struct WordReader<'a> {
    inner: WordReaderInner<'a>,
    read_bytes_buf: &'a mut Box<[Word]>,
}

impl<'a> Read for WordReader<'a> {
    #[inline(always)]
    fn advance(&mut self, bits: usize) {
        self.inner.index += bits;
    }

    #[inline(always)]
    fn peek_bits(&mut self) -> Result<Word> {
        self.inner.reserve_1_to_64_bits(64)?;
        Ok(self.inner.peek_reserved_bits(64))
    }

    #[inline(always)]
    fn read_bit(&mut self) -> Result<bool> {
        self.inner.reserve_1_to_64_bits(1)?;

        let bit_index = self.inner.index;
        self.inner.index += 1;

        let index = bit_index / WORD_BITS;
        let bit_remainder = bit_index % WORD_BITS;

        Ok((self.inner.words[index] & (1 << bit_remainder)) != 0)
    }

    #[inline(always)]
    fn read_bits(&mut self, bits: usize) -> Result<Word> {
        self.inner.reserve_1_to_64_bits(bits)?;
        Ok(self.inner.read_reserved_bits(bits))
    }

    #[inline(never)]
    fn read_bytes(&mut self, len: NonZeroUsize) -> Result<&[u8]> {
        // We read the `[u8]` as `[Word]` and then truncate it.
        let len = len.get();
        let words_len = (len - 1) / WORD_BYTES + 1;
        let src_len = words_len + 1;

        let start = self.inner.index / WORD_BITS;
        let src = if let Some(src) = self.inner.words.get(start..start + src_len) {
            src
        } else {
            return Err(E::Eof.e());
        };

        // Only allocate after src is reserved to prevent memory exhaustion attacks.
        let buf = &mut *self.read_bytes_buf;
        let dst = if let Some(slice) = buf.get_mut(..words_len) {
            slice
        } else {
            alloc_read_bytes_buf(buf, words_len);
            &mut buf[..words_len]
        };

        // If offset is 0 we would shl by 64 which is invalid so we just copy the slice. If shl by
        // 64 resulted in 0 we wouldn't need this special case.
        let offset = self.inner.index % WORD_BITS;
        if offset == 0 {
            let src = &src[..words_len];
            dst.copy_from_slice(src);
        } else {
            let shl = WORD_BITS - offset;
            let shr = offset;

            for (i, w) in dst.iter_mut().enumerate() {
                *w = (src[i] >> shr) | (src[i + 1] << shl);
            }
        }
        self.inner.index += len * u8::BITS as usize;

        // Swap bytes in each word (that was written to) if big endian and bytemuck to bytes.
        if cfg!(target_endian = "big") {
            dst.iter_mut().for_each(|w| *w = w.swap_bytes());
        }
        Ok(&bytemuck::cast_slice(self.read_bytes_buf)[..len])
    }

    #[inline(always)]
    fn reserve_bits(&self, bits: usize) -> Result<()> {
        // TODO could make this overestimate remaining bits by a small amount to simplify logic.
        let whole_words_len = bits / WORD_BITS;
        let words_len = whole_words_len + 1;

        let read = self.inner.index / WORD_BITS + words_len;
        if read >= self.inner.words.len() {
            // TODO hint as unlikely.
            Err(E::Eof.e())
        } else {
            Ok(())
        }
    }
}

#[cold]
fn alloc_read_bytes_buf(buf: &mut Box<[Word]>, len: usize) {
    let new_cap = len.next_power_of_two().max(16);
    *buf = bytemuck::allocation::zeroed_slice_box(new_cap);
}
