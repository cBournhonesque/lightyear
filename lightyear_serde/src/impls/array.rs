use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
};

impl<T: Serde> Serde for &[T] {
    fn ser(&self, writer: &mut dyn BitWrite) {
        for item in *self {
            item.ser(writer);
        }
    }

    fn de(_: &mut BitReader) -> Result<Self, SerdeErr> {
        Err(SerdeErr {})
    }
}

impl<T: Serde, const N: usize> Serde for [T; N] {
    fn ser(&self, writer: &mut dyn BitWrite) {
        for item in self {
            item.ser(writer);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        unsafe {
            let mut to = std::mem::MaybeUninit::<[T; N]>::uninit();
            let top: *mut T = &mut to as *mut std::mem::MaybeUninit<[T; N]> as *mut T;
            for c in 0..N {
                top.add(c).write(Serde::de(reader)?);
            }
            Ok(to.assume_init())
        }
    }
}

// Tests

#[cfg(test)]
mod tests {
    use crate::{
        reader_writer::{BitReader, BitWriter},
        serde::Serde,
    };

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_1: [i32; 4] = [5, 11, 52, 8];
        let in_2: [bool; 3] = [true, false, true];

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1: [i32; 4] = Serde::de(&mut reader).unwrap();
        let out_2: [bool; 3] = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
