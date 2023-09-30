use crate::{
    constants::{MTU_SIZE_BITS, MTU_SIZE_BYTES},
    BitCounter, OutgoingPacket, OwnedBitReader,
};

// TODO: refactor to separate buffer from purely writing into a provided buffer?

// BitWrite
pub trait BitWrite {
    fn write_bit(&mut self, bit: bool);
    fn write_byte(&mut self, byte: u8);
    fn write_bits(&mut self, bits: u32);
    fn is_counter(&self) -> bool;
}

// BitWriter
pub struct BitWriter {
    scratch: u8,
    scratch_index: u8,
    buffer: [u8; MTU_SIZE_BYTES],
    buffer_index: usize,
    current_bits: u32,
    max_bits: u32,
}

impl BitWriter {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            scratch: 0,
            scratch_index: 0,
            buffer: [0; MTU_SIZE_BYTES],
            buffer_index: 0,
            current_bits: 0,
            max_bits: MTU_SIZE_BITS,
        }
    }

    pub fn with_capacity(bit_capacity: u32) -> Self {
        Self {
            scratch: 0,
            scratch_index: 0,
            buffer: [0; MTU_SIZE_BYTES],
            buffer_index: 0,
            current_bits: 0,
            max_bits: bit_capacity,
        }
    }

    fn finalize(&mut self) {
        if self.scratch_index > 0 {
            self.buffer[self.buffer_index] =
                (self.scratch << (8 - self.scratch_index)).reverse_bits();
            self.buffer_index += 1;
        }
        self.max_bits = 0;
    }

    pub fn to_packet(mut self) -> OutgoingPacket {
        self.finalize();
        OutgoingPacket::new(self.buffer_index, self.buffer)
    }

    pub fn to_owned_reader(mut self) -> OwnedBitReader {
        self.finalize();
        OwnedBitReader::new(&self.buffer[0..self.buffer_index])
    }

    pub fn to_bytes(mut self) -> Box<[u8]> {
        self.finalize();
        Box::from(&self.buffer[0..self.buffer_index])
    }

    pub fn counter(&self) -> BitCounter {
        return BitCounter::new(self.current_bits, self.current_bits, self.max_bits);
    }

    pub fn reserve_bits(&mut self, bits: u32) {
        self.max_bits -= bits;
    }

    pub fn release_bits(&mut self, bits: u32) {
        self.max_bits += bits;
    }

    pub fn bits_free(&self) -> u32 {
        self.max_bits - self.current_bits
    }
}

impl BitWrite for BitWriter {
    fn write_bit(&mut self, bit: bool) {
        if self.current_bits >= self.max_bits {
            panic!("Write overflow!");
        }
        self.scratch <<= 1;

        if bit {
            self.scratch |= 1;
        }

        self.scratch_index += 1;
        self.current_bits += 1;

        if self.scratch_index >= 8 {
            self.buffer[self.buffer_index] = self.scratch.reverse_bits();

            self.buffer_index += 1;
            self.scratch_index -= 8;
            self.scratch = 0;
        }
    }

    fn write_byte(&mut self, byte: u8) {
        let mut temp = byte;
        for _ in 0..8 {
            self.write_bit(temp & 1 != 0);
            temp >>= 1;
        }
    }

    fn write_bits(&mut self, _: u32) {
        panic!("This method should not be called for BitWriter!");
    }

    fn is_counter(&self) -> bool {
        false
    }
}

mod tests {

    #[test]
    fn read_write_1_bit() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_bit(true);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_3_bits() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(true);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_8_bits() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_13_bits() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(true);

        writer.write_bit(true);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_16_bits() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(true);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(true);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());

        assert!(reader.read_bit().unwrap());
        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_1_byte() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_byte(123);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert_eq!(123, reader.read_byte().unwrap());
    }

    #[test]
    fn read_write_5_bytes() {
        use crate::{
            bit_reader::BitReader,
            bit_writer::{BitWrite, BitWriter},
        };

        let mut writer = BitWriter::new();

        writer.write_byte(48);
        writer.write_byte(151);
        writer.write_byte(62);
        writer.write_byte(34);
        writer.write_byte(2);

        let buffer = writer.to_bytes();

        let mut reader = BitReader::new(&buffer);

        assert_eq!(48, reader.read_byte().unwrap());
        assert_eq!(151, reader.read_byte().unwrap());
        assert_eq!(62, reader.read_byte().unwrap());
        assert_eq!(34, reader.read_byte().unwrap());
        assert_eq!(2, reader.read_byte().unwrap());
    }
}
