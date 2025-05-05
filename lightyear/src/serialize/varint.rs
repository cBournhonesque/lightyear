

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
        // NOTE: cannot use a value that is close to u64::MAX
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


#[cfg(test)]
mod tests {
    use crate::serialize::reader::{ReadVarInt};
    use crate::serialize::writer::WriteInteger;
    use no_std_io2::io;
    #[cfg(not(feature = "std"))]
    use {
        alloc::vec,
    };

    #[test]
    fn test_varint_len_1() {
        // TEST WITH 1
        let mut writer = vec![];

        let val = 1;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 1);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);

        // TEST WITH 63
        let mut writer = vec![];

        let val = 63;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 1);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }

    #[test]
    fn test_varint_len_2() {
        let mut writer = vec![];

        let val = 64;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 2);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);

        let mut writer = vec![];

        let val = 16383;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 2);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }

    #[test]
    fn test_varint_len_4() {
        let mut writer = vec![];

        let val = 16384;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 4);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);

        let mut writer = vec![];

        let val = 1_073_741_823;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 4);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }

    #[test]
    fn test_varint_len_8() {
        let mut writer = vec![];

        let val = 1_073_741_828;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 8);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);

        let mut writer = vec![];

        let val = 4_611_686_018_427_387_903;
        writer.write_varint(val).unwrap();
        assert_eq!(writer.len(), 8);

        let mut reader = io::Cursor::new(writer);
        let read_val = reader.read_varint().unwrap();
        assert_eq!(val, read_val);
    }
}
