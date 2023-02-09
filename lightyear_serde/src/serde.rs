use super::{
    error::Error,
    reader_writer::{BitReader, BitWrite},
};

/// A trait for objects that can be serialized to a bitstream.
// TODO: rename these as to_writer/from_writer (as per Serde conventions)
pub trait Serde: Sized + Clone + PartialEq {
    /// Serialize Self to a BitWriter
    fn ser(&self, writer: &mut dyn BitWrite);

    /// Parse Self from a BitReader
    fn de(reader: &mut BitReader) -> Result<Self, Error>;
}
