use std::collections::VecDeque;

use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
    UnsignedVariableInteger,
};

impl<T: Serde> Serde for Vec<T> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        length.ser(writer);
        for item in self {
            item.ser(writer);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let length_int = UnsignedVariableInteger::<5>::de(reader)?;
        let length_usize = length_int.get() as usize;
        let mut output: Vec<T> = Vec::with_capacity(length_usize);
        for _ in 0..length_usize {
            output.push(T::de(reader)?)
        }
        Ok(output)
    }
}

impl<T: Serde> Serde for VecDeque<T> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        length.ser(writer);
        for item in self {
            item.ser(writer);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let length_int = UnsignedVariableInteger::<5>::de(reader)?;
        let length_usize = length_int.get() as usize;
        let mut output: VecDeque<T> = VecDeque::with_capacity(length_usize);
        for _ in 0..length_usize {
            output.push_back(T::de(reader)?)
        }
        Ok(output)
    }
}

// Tests

#[cfg(test)]
mod tests {
    use crate::{
        reader_writer::{BitReader, BitWriter},
        serde::Serde,
    };
    use std::collections::VecDeque;

    #[test]
    fn read_write_vec() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = vec![5, 3, 2, 7];
        let in_2 = vec![false, false, true, false, true, true, false, true];

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1: Vec<i32> = Serde::de(&mut reader).unwrap();
        let out_2: Vec<bool> = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }

    #[test]
    fn read_write_vec_deque() {
        // Write
        let mut writer = BitWriter::new();

        let mut in_1 = VecDeque::<i32>::new();
        in_1.push_back(5);
        in_1.push_back(2);
        in_1.push_back(-7);
        in_1.push_back(331);
        in_1.push_back(-527);
        let mut in_2 = VecDeque::<bool>::new();
        in_2.push_back(true);
        in_2.push_back(false);
        in_2.push_back(false);
        in_2.push_back(true);
        in_2.push_back(false);
        in_2.push_back(true);
        in_2.push_back(true);
        in_2.push_back(true);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1: VecDeque<i32> = Serde::de(&mut reader).unwrap();
        let out_2: VecDeque<bool> = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
