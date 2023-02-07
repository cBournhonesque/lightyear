use crate::constants::{MTU_SIZE_BITS, MTU_SIZE_BYTES};
use crate::SerdeErr;

// BitWrite
pub trait BitWrite {
    fn write_bit(&mut self, bit: bool);
    fn write_byte(&mut self, byte: u8);
}

// BitCounter
pub struct BitCounter {
    start_bits: u16,
    current_bits: u16,
    max_bits: u16,
}

impl BitCounter {
    pub fn overflowed(&self) -> bool {
        self.current_bits > self.max_bits
    }

    pub fn bits_needed(&self) -> u16 {
        self.current_bits - self.start_bits
    }
}

impl BitWrite for BitCounter {
    fn write_bit(&mut self, _: bool) {
        self.current_bits += 1;
    }
    fn write_byte(&mut self, _: u8) {
        self.current_bits += 8;
    }
}

// BitWriter
pub struct BitWriter {
    scratch: u8,
    scratch_index: u8,
    buffer: [u8; MTU_SIZE_BYTES],
    buffer_index: usize,
    current_bits: u16,
    max_bits: u16,
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

    pub fn flush(&mut self) -> (usize, [u8; MTU_SIZE_BYTES]) {
        if self.scratch_index > 0 {
            self.buffer[self.buffer_index] =
                (self.scratch << (8 - self.scratch_index)).reverse_bits();
            self.buffer_index += 1;
        }

        let output_length = self.buffer_index;

        self.buffer_index = 0;
        self.scratch_index = 0;
        self.scratch = 0;
        self.current_bits = 0;
        self.max_bits = MTU_SIZE_BITS;

        let mut output_buffer = [0; MTU_SIZE_BYTES];
        output_buffer.clone_from_slice(&self.buffer[0..MTU_SIZE_BYTES]);

        (output_length, output_buffer)
    }

    pub fn counter(&self) -> BitCounter {
        return BitCounter {
            start_bits: self.current_bits,
            current_bits: self.current_bits,
            max_bits: self.max_bits,
        };
    }

    pub fn reserve_bits(&mut self, bits: u16) {
        self.max_bits -= bits;
    }

    pub fn release_bits(&mut self, bits: u16) {
        self.max_bits += bits;
    }

    pub fn bits_free(&self) -> u16 {
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
}

// BitReader

pub struct BitReader<'b> {
    state: BitReaderState,
    buffer: &'b [u8],
}

impl<'b> BitReader<'b> {
    pub fn new(buffer: &'b [u8]) -> Self {
        Self {
            state: BitReaderState {
                scratch: 0,
                scratch_index: 0,
                buffer_index: 0,
            },
            buffer,
        }
    }

    pub fn to_owned(&self) -> OwnedBitReader {
        OwnedBitReader {
            state: self.state,
            buffer: self.buffer.into(),
        }
    }

    pub(crate) fn read_bit(&mut self) -> Result<bool, SerdeErr> {
        if self.state.scratch_index == 0 {
            if self.state.buffer_index == self.buffer.len() {
                return Err(SerdeErr);
            }

            self.state.scratch = self.buffer[self.state.buffer_index];

            self.state.buffer_index += 1;
            self.state.scratch_index += 8;
        }

        let value = self.state.scratch & 1;

        self.state.scratch >>= 1;

        self.state.scratch_index -= 1;

        Ok(value != 0)
    }

    pub(crate) fn read_byte(&mut self) -> Result<u8, SerdeErr> {
        let mut output = 0;
        for _ in 0..7 {
            if self.read_bit()? {
                output |= 128;
            }
            output >>= 1;
        }
        if self.read_bit()? {
            output |= 128;
        }
        Ok(output)
    }
}

// OwnedBitReader

pub struct OwnedBitReader {
    state: BitReaderState,
    buffer: Box<[u8]>,
}

impl OwnedBitReader {
    pub fn new(buffer: &[u8]) -> Self {
        Self {
            state: BitReaderState {
                scratch: 0,
                scratch_index: 0,
                buffer_index: 0,
            },
            buffer: buffer.into(),
        }
    }

    pub fn borrow(&self) -> BitReader {
        BitReader {
            state: self.state,
            buffer: &self.buffer,
        }
    }
}

// BitReaderState
#[derive(Copy, Clone)]
struct BitReaderState {
    scratch: u8,
    scratch_index: u8,
    buffer_index: usize,
}

mod tests {

    #[test]
    fn read_write_1_bit() {
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

        let mut writer = BitWriter::new();

        writer.write_bit(true);

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_3_bits() {
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(true);

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        assert!(!reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
        assert!(reader.read_bit().unwrap());
    }

    #[test]
    fn read_write_8_bits() {
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

        let mut writer = BitWriter::new();

        writer.write_bit(false);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(true);

        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bit(false);
        writer.write_bit(false);

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

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
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

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

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

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
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

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

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

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
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

        let mut writer = BitWriter::new();

        writer.write_byte(123);

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        assert_eq!(123, reader.read_byte().unwrap());
    }

    #[test]
    fn read_write_5_bytes() {
        use crate::reader_writer::{BitReader, BitWrite, BitWriter};

        let mut writer = BitWriter::new();

        writer.write_byte(48);
        writer.write_byte(151);
        writer.write_byte(62);
        writer.write_byte(34);
        writer.write_byte(2);

        let (buffer_length, buffer) = writer.flush();

        let mut reader = BitReader::new(&buffer[..buffer_length]);

        assert_eq!(48, reader.read_byte().unwrap());
        assert_eq!(151, reader.read_byte().unwrap());
        assert_eq!(62, reader.read_byte().unwrap());
        assert_eq!(34, reader.read_byte().unwrap());
        assert_eq!(2, reader.read_byte().unwrap());
    }
}
