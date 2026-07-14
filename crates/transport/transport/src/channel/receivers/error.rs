//! Errors for receiving packets

pub type Result<T> = core::result::Result<T, ChannelReceiveError>;
#[derive(thiserror::Error, Debug)]
pub enum ChannelReceiveError {
    #[error("A message was received without a message ID")]
    MissingMessageId,
    #[error("fragmented message declared an invalid fragment count: {num_fragments}")]
    InvalidFragmentCount { num_fragments: u64 },
    #[error("fragmented message declared an invalid fragment size: {fragment_size}")]
    InvalidFragmentSize { fragment_size: u64 },
    #[error("fragment index {fragment_index} is outside fragment count {num_fragments}")]
    InvalidFragmentIndex {
        fragment_index: u64,
        num_fragments: u64,
    },
    #[error("fragment count changed while reassembling message: expected {expected}, got {actual}")]
    FragmentCountMismatch { expected: usize, actual: usize },
    #[error("fragment size changed while reassembling message: expected {expected}, got {actual}")]
    FragmentSizeMismatch { expected: usize, actual: usize },
    #[error("fragmented message size overflows the local address space")]
    FragmentedMessageSizeOverflow,
    #[error(
        "fragment compression changed while reassembling message: expected {expected}, got {actual}"
    )]
    FragmentCompressionMismatch {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("fragmented message completed before receiving compression metadata")]
    MissingFragmentCompression,
    #[error("fragment uses unsupported compression: {compression}")]
    UnsupportedFragmentCompression { compression: &'static str },
    #[error("compressed fragment payload could not be decompressed")]
    FragmentDecompressionFailed,
    #[error("decompressed fragment payload size {actual} exceeds configured limit {limit}")]
    FragmentDecompressedPayloadTooLarge { actual: usize, limit: usize },
    #[error("non-final fragment has size {actual}, expected {expected}")]
    InvalidNonFinalFragmentSize { actual: usize, expected: usize },
    #[error("final fragment has size {actual}, maximum {max}")]
    InvalidFinalFragmentSize { actual: usize, max: usize },
}
