use crate::{
    bit_reader::BitReader,
    bit_writer::BitWrite,
    error::SerdeErr,
    serde::{ConstBitLength, Serde},
};

// Unit //

impl Serde for () {
    fn ser(&self, _: &mut dyn BitWrite) {}

    fn de(_: &mut BitReader) -> Result<Self, SerdeErr> {
        Ok(())
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for () {
    fn const_bit_length() -> u32 {
        0
    }
}

// tests

#[cfg(test)]
mod unit_tests {
    use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_unit = ();

        in_unit.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_unit = Serde::de(&mut reader).unwrap();

        assert_eq!(in_unit, out_unit);
    }
}

// Boolean //

impl Serde for bool {
    fn ser(&self, writer: &mut dyn BitWrite) {
        writer.write_bit(*self);
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        reader.read_bit()
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for bool {
    fn const_bit_length() -> u32 {
        1
    }
}

// tests

#[cfg(test)]
mod bool_tests {
    use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = true;
        let in_2 = false;

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}

// Characters //

impl Serde for char {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let u32char = *self as u32;
        let bytes = unsafe { std::mem::transmute::<&u32, &[u8; 4]>(&u32char) };
        for byte in bytes {
            writer.write_byte(*byte);
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let mut bytes = [0_u8; 4];
        for byte in &mut bytes {
            *byte = reader.read_byte()?;
        }
        let mut container = [0_u32];
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr().offset(0_isize) as *const u32,
                container.as_mut_ptr() as *mut u32,
                1,
            )
        }

        if let Some(inner_char) = char::from_u32(container[0]) {
            Ok(inner_char)
        } else {
            Err(SerdeErr {})
        }
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for char {
    fn const_bit_length() -> u32 {
        <[u8; 4] as ConstBitLength>::const_bit_length()
    }
}

// tests

#[cfg(test)]
mod char_tests {
    use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};

    #[test]
    fn read_write() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = 'O';
        let in_2 = '!';

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);

        let buffer = writer.to_bytes();

        //Read
        let mut reader = BitReader::new(&buffer);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
    }
}

// Integers & Floating-point Numbers //

macro_rules! impl_serde_for {
    ($impl_type:ident) => {
        impl Serde for $impl_type {
            fn ser(&self, writer: &mut dyn BitWrite) {
                let du8 = unsafe {
                    std::mem::transmute::<&$impl_type, &[u8; std::mem::size_of::<$impl_type>()]>(
                        &self,
                    )
                };
                for byte in du8 {
                    writer.write_byte(*byte);
                }
            }

            fn de(reader: &mut BitReader) -> Result<$impl_type, SerdeErr> {
                const BYTES_LENGTH: usize = std::mem::size_of::<$impl_type>();
                let mut byte_array = [0_u8; BYTES_LENGTH];
                for index in 0..BYTES_LENGTH {
                    byte_array[index] = reader.read_byte()?;
                }
                let mut container = [0 as $impl_type];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        byte_array.as_ptr() as *const $impl_type,
                        container.as_mut_ptr() as *mut $impl_type,
                        1,
                    )
                }
                Ok(container[0])
            }

            fn bit_length(&self) -> u32 {
                <Self as ConstBitLength>::const_bit_length()
            }
        }
        impl ConstBitLength for $impl_type {
            fn const_bit_length() -> u32 {
                const BYTES_LENGTH: u32 = std::mem::size_of::<$impl_type>() as u32;
                return BYTES_LENGTH * 8;
            }
        }
    };
}

// number primitives
impl_serde_for!(u16);
impl_serde_for!(u32);
impl_serde_for!(u64);
impl_serde_for!(i16);
impl_serde_for!(i32);
impl_serde_for!(i64);
impl_serde_for!(f32);
impl_serde_for!(f64);

// u8
impl Serde for u8 {
    fn ser(&self, writer: &mut dyn BitWrite) {
        writer.write_byte(*self);
    }

    fn de(reader: &mut BitReader) -> Result<u8, SerdeErr> {
        reader.read_byte()
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for u8 {
    fn const_bit_length() -> u32 {
        8
    }
}

// i8
impl Serde for i8 {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let du8 = unsafe { std::mem::transmute::<&i8, &u8>(self) };
        writer.write_byte(*du8);
    }

    fn de(reader: &mut BitReader) -> Result<i8, SerdeErr> {
        let byte = [reader.read_byte()?];
        let mut container = [0_i8];
        unsafe {
            std::ptr::copy_nonoverlapping(
                byte.as_ptr() as *const i8,
                container.as_mut_ptr() as *mut i8,
                1,
            )
        }
        Ok(container[0])
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for i8 {
    fn const_bit_length() -> u32 {
        8
    }
}

// usize
impl Serde for usize {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let u64usize = *self as u64;
        let du8 = unsafe { std::mem::transmute::<&u64, &[u8; 8]>(&u64usize) };
        for byte in du8 {
            writer.write_byte(*byte);
        }
    }

    fn de(reader: &mut BitReader) -> Result<usize, SerdeErr> {
        let mut byte_array = [0_u8; 8];
        for byte in &mut byte_array {
            *byte = reader.read_byte()?;
        }
        let mut container = [0_u64];
        unsafe {
            std::ptr::copy_nonoverlapping(
                byte_array.as_ptr().offset(0_isize) as *const u64,
                container.as_mut_ptr() as *mut u64,
                1,
            )
        }
        Ok(container[0] as usize)
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for usize {
    fn const_bit_length() -> u32 {
        <u64 as ConstBitLength>::const_bit_length()
    }
}

// isize
impl Serde for isize {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let u64usize = *self as u64;
        let du8 = unsafe { std::mem::transmute::<&u64, &[u8; 8]>(&u64usize) };
        for byte in du8 {
            writer.write_byte(*byte);
        }
    }

    fn de(reader: &mut BitReader) -> Result<isize, SerdeErr> {
        let mut byte_array = [0_u8; 8];
        for byte in &mut byte_array {
            *byte = reader.read_byte()?;
        }
        let mut container = [0_u64];
        unsafe {
            std::ptr::copy_nonoverlapping(
                byte_array.as_ptr().offset(0_isize) as *const u64,
                container.as_mut_ptr() as *mut u64,
                1,
            )
        }
        Ok(container[0] as isize)
    }

    fn bit_length(&self) -> u32 {
        <Self as ConstBitLength>::const_bit_length()
    }
}

impl ConstBitLength for isize {
    fn const_bit_length() -> u32 {
        <u64 as ConstBitLength>::const_bit_length()
    }
}

// tests

macro_rules! test_serde_for {
    ($impl_type:ident, $test_name:ident) => {
        #[test]
        fn $test_name() {
            use crate::{bit_reader::BitReader, bit_writer::BitWriter, serde::Serde};

            // Write
            let mut writer = BitWriter::new();

            let in_1: $impl_type = 123 as $impl_type;

            in_1.ser(&mut writer);

            let buffer = writer.to_bytes();

            //Read
            let mut reader = BitReader::new(&buffer);

            let out_1 = Serde::de(&mut reader).unwrap();

            assert_eq!(in_1, out_1);
        }
    };
}

mod number_tests {
    test_serde_for!(u8, test_u8);
    test_serde_for!(u16, test_u16);
    test_serde_for!(u32, test_u32);
    test_serde_for!(u64, test_u64);
    test_serde_for!(usize, test_usize);
    test_serde_for!(i8, test_i8);
    test_serde_for!(i16, test_i16);
    test_serde_for!(i32, test_i32);
    test_serde_for!(i64, test_i64);
    test_serde_for!(isize, test_isize);
    test_serde_for!(f32, test_f32);
    test_serde_for!(f64, test_f64);
}
