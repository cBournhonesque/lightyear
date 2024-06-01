use crate::serialize::SerializationError;
use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
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
        match len {
            1 => self.write_u8(value as u8)?,
            2 => {
                let val = (value as u16) | 0x4000;
                self.write_u16::<NetworkEndian>(val)?;
            }
            4 => {
                let val = (value as u32) | 0x8000_0000;
                self.write_u32::<NetworkEndian>(val)?;
            }
            8 => {
                let val = value | 0xc0_0000_0000_0000;
                self.write_u64::<NetworkEndian>(val)?;
            }
            _ => return Err(std::io::Error::other("value is too large for varint").into()),
        };

        Ok(())
    }
}

impl<T: WriteBytesExt> VarIntWriteExt for T {}

pub trait VarIntReadExt: ReadBytesExt + Seek {
    /// Reads an unsigned variable-length integer in network byte-order from
    /// the current offset and advances the buffer.
    fn read_varint(&mut self) -> Result<u64, SerializationError> {
        let first = self.read_u8()?;

        let len = varint_parse_len(first);
        let out = match len {
            1 => u64::from(first),
            2 => {
                // go back 1 byte because we read the first byte above
                self.seek(std::io::SeekFrom::Current(-1))?;
                u64::from(self.read_u16::<NetworkEndian>()? & 0x3fff)
            }
            4 => {
                self.seek(std::io::SeekFrom::Current(-1))?;
                u64::from(self.read_u32::<NetworkEndian>()? & 0x3fffffff)
            }
            8 => {
                self.seek(std::io::SeekFrom::Current(-1))?;
                self.read_u64::<NetworkEndian>()? & 0x3fffffffffffffff
            }

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
    fn test_varint_len_1() {
        // TEST WITH 1
        let mut writer = vec![];

        let val = 1;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 1);

        let mut reader = Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);

        // TEST WITH 63
        let mut writer = vec![];

        let val = 63;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 1);

        let mut reader = Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }

    #[test]
    fn test_varint_len_2() {
        let mut writer = vec![];

        let val = 64;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 2);

        let mut reader = Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }

    #[test]
    fn test_varint_len_4() {
        let mut writer = vec![];

        let val = 16384;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 4);

        let mut reader = Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }
}
