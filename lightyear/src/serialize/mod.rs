//! Serialization and deserialization of types

pub mod bitcode;
pub(crate) mod octets;
pub mod reader;
pub mod writer;

pub type RawData = Vec<u8>;
