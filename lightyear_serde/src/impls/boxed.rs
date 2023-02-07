use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
};

impl<T: Serde> Serde for Box<T> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        (**self).ser(writer)
    }

    fn de(reader: &mut BitReader) -> Result<Box<T>, SerdeErr> {
        Ok(Box::new(Serde::de(reader)?))
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

        let in_1 = Box::new(123);
        let in_2 = Box::new(true);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Box::<u8>::de(&mut reader).unwrap();
        let out_2 = Box::<bool>::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
