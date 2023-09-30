use super::{bit_reader::BitReader, bit_writer::BitWrite, error::SerdeErr};

/// A trait for objects that can be serialized to a bitstream.
pub trait Serde: Sized + Clone + PartialEq {
    /// Serialize Self to a BitWriter
    fn ser(&self, writer: &mut dyn BitWrite);

    /// Parse Self from a BitReader
    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr>;

    /// Return length of value in bits
    fn bit_length(&self) -> u32;
}

pub trait ConstBitLength {
    fn const_bit_length() -> u32;
}
