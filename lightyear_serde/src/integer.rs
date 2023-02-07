use crate::{
    error::SerdeErr,
    reader_writer::{BitReader, BitWrite},
    serde::Serde,
};

pub type UnsignedInteger<const BITS: u8> = SerdeInteger<false, false, BITS>;
pub type SignedInteger<const BITS: u8> = SerdeInteger<true, false, BITS>;
pub type UnsignedVariableInteger<const BITS: u8> = SerdeInteger<false, true, BITS>;
pub type SignedVariableInteger<const BITS: u8> = SerdeInteger<true, true, BITS>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SerdeInteger<const SIGNED: bool, const VARIABLE: bool, const BITS: u8> {
    inner: i128,
}

impl<const SIGNED: bool, const VARIABLE: bool, const BITS: u8>
    SerdeInteger<SIGNED, VARIABLE, BITS>
{
    pub fn get(&self) -> i128 {
        self.inner
    }

    pub fn new<T: Into<i128>>(value: T) -> Self {
        let inner = Into::<i128>::into(value);

        if inner < 0 && !SIGNED {
            panic!("can't encode a negative number with an Unsigned Integer!");
        }

        if BITS == 0 {
            panic!("can't create an integer with 0 bits...");
        }
        if BITS > 127 {
            panic!("can't create an integer with more than 127 bits...");
        }

        if !VARIABLE {
            let max_value: i128 = 2_i128.pow(BITS as u32);
            if inner >= max_value {
                panic!(
                    "with {} bits, can't encode number greater than {}",
                    BITS, max_value
                );
            }
            if inner < 0 && SIGNED {
                let min_value: i128 = -(2_i128.pow(BITS as u32));
                if inner <= min_value {
                    panic!(
                        "with {} bits, can't encode number less than {}",
                        BITS, min_value
                    );
                }
            }
        }

        Self { inner }
    }

    fn new_unchecked(value: i128) -> Self {
        Self { inner: value }
    }
}

impl<const SIGNED: bool, const VARIABLE: bool, const BITS: u8> Serde
    for SerdeInteger<SIGNED, VARIABLE, BITS>
{
    fn ser(&self, writer: &mut dyn BitWrite) {
        let mut value: u128;
        let negative = self.inner < 0;

        if SIGNED {
            // 1 if negative, 0 if positive
            writer.write_bit(negative);
            if negative {
                value = -self.inner as u128;
            } else {
                value = self.inner as u128;
            }
        } else {
            value = self.inner as u128;
        }

        if VARIABLE {
            let mut proceed;
            loop {
                if value >= 2_u128.pow(BITS as u32) {
                    proceed = true;
                } else {
                    proceed = false;
                }
                writer.write_bit(proceed);

                for _ in 0..BITS {
                    writer.write_bit(value & 1 != 0);
                    value >>= 1;
                }
                if !proceed {
                    return;
                }
            }
        } else {
            for _ in 0..BITS {
                writer.write_bit(value & 1 != 0);
                value >>= 1;
            }
        }
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let mut negative: bool = false;
        if SIGNED {
            negative = reader.read_bit()?;
        }

        if VARIABLE {
            let mut total_bits: usize = 0;
            let mut output: u128 = 0;

            loop {
                let proceed = reader.read_bit()?;

                for _ in 0..BITS {
                    total_bits += 1;

                    output <<= 1;

                    if reader.read_bit()? {
                        output |= 1;
                    }
                }

                if !proceed {
                    output <<= 128 - total_bits;
                    output = output.reverse_bits();

                    let value: i128 = output as i128;
                    if negative {
                        return Ok(SerdeInteger::new_unchecked(-value));
                    } else {
                        return Ok(SerdeInteger::new_unchecked(value));
                    }
                }
            }
        } else {
            let mut output: u128 = 0;

            for _ in 0..BITS {
                output <<= 1;

                if reader.read_bit()? {
                    output |= 1;
                }
            }

            output <<= 128 - BITS;
            output = output.reverse_bits();

            let value: i128 = output as i128;
            if negative {
                Ok(SerdeInteger::new_unchecked(-value))
            } else {
                Ok(SerdeInteger::new_unchecked(value))
            }
        }
    }
}

// Tests

#[cfg(test)]
mod tests {
    use crate::{
        integer::{SignedInteger, SignedVariableInteger, UnsignedInteger, UnsignedVariableInteger},
        reader_writer::{BitReader, BitWriter},
        serde::Serde,
    };

    #[test]
    fn in_and_out() {
        let in_u16: u16 = 123;
        let middle = UnsignedInteger::<9>::new(in_u16);
        let out_u16: u16 = middle.get() as u16;

        assert_eq!(in_u16, out_u16);
    }

    #[test]
    fn read_write_unsigned() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = UnsignedInteger::<7>::new(123);
        let in_2 = UnsignedInteger::<20>::new(535221);
        let in_3 = UnsignedInteger::<2>::new(3);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);
        in_3.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();
        let out_3 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
        assert_eq!(in_3, out_3);
    }

    #[test]
    fn read_write_signed() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = SignedInteger::<10>::new(-668);
        let in_2 = SignedInteger::<20>::new(53);
        let in_3 = SignedInteger::<2>::new(-3);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);
        in_3.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();
        let out_3 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
        assert_eq!(in_3, out_3);
    }

    #[test]
    fn read_write_unsigned_variable() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = UnsignedVariableInteger::<3>::new(23);
        let in_2 = UnsignedVariableInteger::<5>::new(153);
        let in_3 = UnsignedVariableInteger::<2>::new(3);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);
        in_3.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();
        let out_3 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
        assert_eq!(in_3, out_3);
    }

    #[test]
    fn read_write_signed_variable() {
        // Write
        let mut writer = BitWriter::new();

        let in_1 = SignedVariableInteger::<5>::new(-668);
        let in_2 = SignedVariableInteger::<6>::new(53735);
        let in_3 = SignedVariableInteger::<2>::new(-3);

        in_1.ser(&mut writer);
        in_2.ser(&mut writer);
        in_3.ser(&mut writer);

        let (buffer_length, buffer) = writer.flush();

        // Read

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        let out_1 = Serde::de(&mut reader).unwrap();
        let out_2 = Serde::de(&mut reader).unwrap();
        let out_3 = Serde::de(&mut reader).unwrap();

        assert_eq!(in_1, out_1);
        assert_eq!(in_2, out_2);
        assert_eq!(in_3, out_3);
    }
}
