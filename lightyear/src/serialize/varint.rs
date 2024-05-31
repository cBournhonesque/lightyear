use crate::serialize::SerializationError;
use byteorder::{ReadBytesExt, WriteBytesExt};
use std::io::Seek;

/// Returns how many bytes it would take to encode `v` as a variable-length
/// integer.
///
/// SAFETY: panics if you use a varint that is bigger than 8 bytes
pub const fn varint_len(v: u64) -> usize {
    if v <= 63 {
        1
    } else if v <= 16383 {
        2
    } else if v <= 1_073_741_823 {
        4
    } else if v <= 4_611_686_018_427_387_903 {
        8
    } else {
        unreachable!()
    }
}

/// Returns how long the variable-length integer is, given its first byte.
pub const fn varint_parse_len(first: u8) -> usize {
    match first >> 6 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => unreachable!(),
    }
}

pub trait VarIntWriteExt: WriteBytesExt {
    /// Write a variable length integer to the writer, in network byte order
    fn write_varint(&mut self, value: u64) -> Result<(), SerializationError> {
        let len = varint_len(value);
        let buf = match len {
            1 => self.write_u8(value as u8)?,
            2 => {
                let buf = self.write_u16(value as u16)?;
                buf[0] |= 0x40;
                buf
            }
            4 => {
                let buf = self.write_u32(value as u32)?;
                buf[0] |= 0x80;
                buf
            }
            8 => {
                let buf = self.write_u64(value)?;
                buf[0] |= 0xc0;
                buf
            }
            _ => return Err(std::io::Error::other("value is too large for varint").into()),
        };

        Ok(buf)
    }
}

impl<T: WriteBytesExt> VarIntWriteExt for T {}

pub trait VarIntReadExt: ReadBytesExt + Seek {
    /// Reads an unsigned variable-length integer in network byte-order from
    /// the current offset and advances the buffer.
    fn read_varint(&mut self) -> Result<u64, SerializationError> {
        let first = self.read_u8()?;
        self.seek(std::io::SeekFrom::Current(-1))?;
        let len = varint_parse_len(first);
        let out = match len {
            1 => u64::from(first),

            2 => u64::from(self.read_u16()? & 0x3fff),

            4 => u64::from(self.read_u32()? & 0x3fffffff),

            8 => self.read_u64()? & 0x3fffffffffffffff,

            _ => return Err(std::io::Error::other("value is too large for varint").into()),
        };
        Ok(out)
    }
}

impl<T: ReadBytesExt + Seek> VarIntReadExt for T {}

#[cfg(test)]
mod tests {
    use crate::serialize::varint::{VarIntReadExt, VarIntWriteExt};
    use std::io::Cursor;

    #[test]
    fn test_varint() {
        let mut writer = Cursor::new(vec![]);

        let val = 63 as u64;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.position(), 1);

        let mut reader = writer;
        reader.set_position(0);

        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }
}
