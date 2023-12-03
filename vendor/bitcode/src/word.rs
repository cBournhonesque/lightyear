/// How much data is copied in write_bits/read_bits.
/// Can't be changed to another size without significant code changes.
pub type Word = u64;
pub const WORD_BITS: usize = Word::BITS as usize;
pub const WORD_BYTES: usize = std::mem::size_of::<Word>();
