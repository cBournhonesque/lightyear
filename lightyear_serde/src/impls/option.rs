use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
};

impl<T: Serde> Serde for Option<T> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        if let Some(value) = self {
            writer.write_bit(true);
            value.ser(writer);
        } else {
            writer.write_bit(false);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Option<T>, SerdeErr> {
        if reader.read_bit()? {
            Ok(Some(T::de(reader)?))
        } else {
            Ok(None)
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

        let in_1 = Some(123);
        let in_2: Option<f32> = None;

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Option::<u8>::de(&mut reader).unwrap();
        let out_2 = Option::<f32>::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
