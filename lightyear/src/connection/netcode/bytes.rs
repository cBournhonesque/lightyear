use byteorder::{ReadBytesExt, WriteBytesExt};

pub trait Bytes: Sized {
    const SIZE: usize = core::mem::size_of::<Self>();
    type Error;
    fn write_to(&self, writer: &mut impl WriteBytesExt) -> Result<(), Self::Error>;
    fn read_from(reader: &mut impl ReadBytesExt) -> Result<Self, Self::Error>;
}
