use crate::{
    bit_reader::BitReader,
    bit_writer::BitWrite,
    error::SerdeErr,
    serde::{ConstBitLength, Serde},
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

    fn bit_length(&self) -> u32 {
        let mut output = 1;
        if let Some(value) = self {
            output += value.bit_length();
        }
        output
    }
}

impl<T: ConstBitLength> ConstBitLength for Option<T> {
    fn const_bit_length() -> u32 {
        return 1 + T::const_bit_length();
    }
}

// Tests

#[cfg(test)]
mod tests {
    use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = Some(123);
        let in_2: Option<f32> = None;

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_1 = Option::<u8>::de(&mut reader).unwrap();
        let out_2 = Option::<f32>::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
