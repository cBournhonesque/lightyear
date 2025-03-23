use crate::serialize::reader::ReadInteger;
use crate::serialize::writer::{WriteInteger};

pub trait Bytes: Sized {
    const SIZE: usize = core::mem::size_of::<Self>();
    type Error;
    fn write_to(&self, writer: &mut impl WriteInteger) -> Result<(), Self::Error>;
    fn read_from(reader: &mut impl ReadInteger) -> Result<Self, Self::Error>;
}
