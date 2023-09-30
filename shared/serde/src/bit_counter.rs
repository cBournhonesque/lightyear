use crate::BitWrite;

// BitCounter
pub struct BitCounter {
    start_bits: u32,
    current_bits: u32,
    max_bits: u32,
}

impl BitCounter {
    pub fn new(start_bits: u32, current_bits: u32, max_bits: u32) -> Self {
        Self {
            start_bits,
            current_bits,
            max_bits,
        }
    }

    pub fn overflowed(&self) -> bool {
        self.current_bits > self.max_bits
    }

    pub fn bits_needed(&self) -> u32 {
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
    fn write_bits(&mut self, bits: u32) {
        self.current_bits += bits;
    }
    fn is_counter(&self) -> bool {
        true
    }
}
