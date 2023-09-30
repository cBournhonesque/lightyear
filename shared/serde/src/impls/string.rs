use crate::{
    bit_reader::BitReader, bit_writer::BitWrite, error::SerdeErr, serde::Serde,
    UnsignedVariableInteger,
};

impl Serde for String {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let length = UnsignedVariableInteger::<9>::new(self.len() as u64);
        length.ser(writer);
        let bytes = self.as_bytes();
        for byte in bytes {
            writer.write_byte(*byte);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let length_int = UnsignedVariableInteger::<9>::de(reader)?;
        let length_usize = length_int.get() as usize;
        let mut bytes: Vec<u8> = Vec::with_capacity(length_usize);
        for _ in 0..length_usize {
            bytes.push(reader.read_byte()?);
        }

        let result = String::from_utf8_lossy(&bytes).into_owned();
        Ok(result)
    }

    fn bit_length(&self) -> u32 {
        let mut output = 0;
        let length = UnsignedVariableInteger::<9>::new(self.len() as u64);
        output += length.bit_length();
        output += (self.len() as u32) * 8;
        output
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

        let in_1 = "Hello world!".to_string();
        let in_2 = "This is a string.".to_string();

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        // Read
        let mut reader = BitReader::new(&buffer);

        let out_1: String = Serde::de(&mut reader).unwrap();
        let out_2: String = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
