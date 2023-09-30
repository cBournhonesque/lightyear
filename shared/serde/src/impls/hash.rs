use crate::{
    bit_reader::BitReader, bit_writer::BitWrite, error::SerdeErr, serde::Serde,
    UnsignedVariableInteger,
};
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

impl<K: Serde + Eq + Hash> Serde for HashSet<K> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        length.ser(writer);
        for value in self {
            value.ser(writer);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let length_int = UnsignedVariableInteger::<5>::de(reader)?;
        let length_usize = length_int.get() as usize;
        let mut output: HashSet<K> = HashSet::new();
        for _ in 0..length_usize {
            let value = K::de(reader)?;
            output.insert(value);
        }
        Ok(output)
    }

    fn bit_length(&self) -> u32 {
        let mut output = 0;
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        output += length.bit_length();
        for value in self {
            output += value.bit_length();
        }
        output
    }
}

impl<K: Serde + Eq + Hash, V: Serde> Serde for HashMap<K, V> {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        length.ser(writer);
        for (key, value) in self {
            key.ser(writer);
            value.ser(writer);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let length_int = UnsignedVariableInteger::<5>::de(reader)?;
        let length_usize = length_int.get() as usize;
        let mut output: HashMap<K, V> = HashMap::new();
        for _ in 0..length_usize {
            let key = K::de(reader)?;
            let value = V::de(reader)?;
            output.insert(key, value);
        }
        Ok(output)
    }

    fn bit_length(&self) -> u32 {
        let mut output = 0;
        let length = UnsignedVariableInteger::<5>::new(self.len() as u64);
        output += length.bit_length();
        for (key, value) in self {
            output += key.bit_length();
            output += value.bit_length();
        }
        output
    }
}

// Tests

#[cfg(test)]
mod tests {
    use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn read_write_hash_map() {
        // Write
        let mut writer = BitWriter::new();

        let mut in_1 = HashMap::<i32, String>::new();
        in_1.insert(-7, "negative seven".to_string());
        in_1.insert(331, "three hundred and thiry-one".to_string());
        in_1.insert(-65, "negative sixty-five".to_string());
        let mut in_2 = HashMap::<u16, bool>::new();
        in_2.insert(5, true);
        in_2.insert(73, false);
        in_2.insert(44, false);
        in_2.insert(21, true);
        in_2.insert(67, false);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_1 = HashMap::<i32, String>::de(&mut reader).unwrap();
        let out_2 = HashMap::<u16, bool>::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }

    #[test]
    fn read_write_hash_set() {
        // Write
        let mut writer = BitWriter::new();

        let mut in_1 = HashSet::<i32>::new();
        in_1.insert(-7);
        in_1.insert(331);
        in_1.insert(-65);
        let mut in_2 = HashSet::<u16>::new();
        in_2.insert(5);
        in_2.insert(73);
        in_2.insert(44);
        in_2.insert(21);
        in_2.insert(67);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_1 = HashSet::<i32>::de(&mut reader).unwrap();
        let out_2 = HashSet::<u16>::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}
