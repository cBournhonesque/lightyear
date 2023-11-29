//! Contains a set of useful utilities

/// Name of a struct of type
pub mod named;

/// Wrapper around a heap
pub(crate) mod ready_buffer;

/// Wrapper around a list where the index is a wrapping key
pub(crate) mod sequence_buffer;

/// u16 that wraps around when it reaches the maximum value
pub mod wrapping_id;
