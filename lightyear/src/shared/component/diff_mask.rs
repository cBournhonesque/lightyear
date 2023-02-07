use std::fmt;

// The DiffMask is a variable-length byte array, where each bit represents
// the current state of a Property owned by a Replica.
// The Property tracks whether it has been updated and needs to be synced
// with the remote Client
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DiffMask {
    mask: Vec<u8>,
}

impl DiffMask {
    /// Create a new DiffMask with a given number of bytes
    pub fn new(bytes: u8) -> DiffMask {
        DiffMask {
            mask: vec![0; bytes as usize],
        }
    }

    /// Gets the bit at the specified position within the DiffMask
    pub fn bit(&self, index: u8) -> Option<bool> {
        if let Some(byte) = self.mask.get((index / 8) as usize) {
            let adjusted_index = index % 8;
            return Some(byte & (1 << adjusted_index) != 0);
        }

        None
    }

    /// Sets the bit at the specified position within the DiffMask
    pub fn set_bit(&mut self, index: u8, value: bool) {
        if let Some(byte) = self.mask.get_mut((index / 8) as usize) {
            let adjusted_index = index % 8;
            let bit_mask = 1 << adjusted_index;
            if value {
                *byte |= bit_mask;
            } else {
                *byte &= !bit_mask;
            }
        }
    }

    /// Clears the whole DiffMask
    pub fn clear(&mut self) {
        let size = self.mask.len();
        self.mask = vec![0; size];
    }

    /// Returns whether any bit has been set in the DiffMask
    pub fn is_clear(&self) -> bool {
        for byte in self.mask.iter() {
            if *byte != 0 {
                return false;
            }
        }
        true
    }

    /// Get the number of bytes required to represent the DiffMask
    pub fn byte_number(&self) -> u8 {
        self.mask.len() as u8
    }

    /// Gets a byte at the specified index in the DiffMask
    pub fn byte(&self, index: usize) -> u8 {
        self.mask[index]
    }

    /// Performs a NAND operation on the DiffMask, with another DiffMask
    pub fn nand(&mut self, other: &DiffMask) {
        //if other diff mask has different capacity, do nothing
        if other.byte_number() != self.byte_number() {
            return;
        }

        for n in 0..self.mask.len() {
            if let Some(my_byte) = self.mask.get_mut(n) {
                let other_byte = !other.byte(n);
                *my_byte &= other_byte;
            }
        }
    }

    /// Performs an OR operation on the DiffMask, with another DiffMask
    pub fn or(&mut self, other: &DiffMask) {
        //if other diff mask has different capacity, do nothing
        if other.byte_number() != self.byte_number() {
            return;
        }

        for n in 0..self.mask.len() {
            if let Some(my_byte) = self.mask.get_mut(n) {
                let other_byte = other.byte(n);
                *my_byte |= other_byte;
            }
        }
    }

    /// Copies the DiffMask into another DiffMask
    pub fn copy_contents(&mut self, other: &DiffMask) {
        //if other diff mask has different capacity, do nothing
        if other.byte_number() != self.byte_number() {
            return;
        }

        for n in 0..self.mask.len() {
            if let Some(my_byte) = self.mask.get_mut(n) {
                let other_byte = other.byte(n);
                *my_byte = other_byte;
            }
        }
    }
}

impl fmt::Display for DiffMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out_string: String = String::new();
        for y in 0..8 {
            if let Some(bit) = self.bit(y) {
                if bit {
                    out_string.push('1');
                } else {
                    out_string.push('0');
                }
            }
        }
        write!(f, "{}", out_string)
    }
}

#[cfg(test)]
mod single_byte_tests {
    use crate::shared::DiffMask;

    #[test]
    fn getset() {
        let mut mask = DiffMask::new(1);

        mask.set_bit(0, true);
        mask.set_bit(2, true);
        mask.set_bit(4, true);
        mask.set_bit(6, true);
        mask.set_bit(4, false);

        assert!(mask.bit(0).unwrap());
        assert!(!mask.bit(1).unwrap());
        assert!(mask.bit(2).unwrap());
        assert!(!mask.bit(4).unwrap());
        assert!(mask.bit(6).unwrap());
    }

    #[test]
    fn clear() {
        let mut mask = DiffMask::new(1);

        mask.set_bit(0, true);
        mask.set_bit(2, true);
        mask.set_bit(4, true);
        mask.set_bit(6, true);

        mask.clear();

        assert!(!mask.bit(0).unwrap());
        assert!(!mask.bit(2).unwrap());
        assert!(!mask.bit(4).unwrap());
        assert!(!mask.bit(6).unwrap());
    }

    #[test]
    fn is_clear_true() {
        let mut mask = DiffMask::new(1);

        mask.set_bit(2, true);

        assert!(!mask.is_clear());

        mask.set_bit(2, false);

        assert!(mask.is_clear());
    }

    #[test]
    fn bytes() {
        let mask = DiffMask::new(1);
        assert!(mask.byte_number() == 1);
    }

    #[test]
    fn byte() {
        let mut mask = DiffMask::new(1);
        mask.set_bit(2, true);
        let byte = mask.byte(0);
        assert!(byte == 4);
    }

    #[test]
    fn nand() {
        let mut mask_a = DiffMask::new(1);
        mask_a.set_bit(1, true);
        mask_a.set_bit(2, true);

        let mut mask_b = DiffMask::new(1);
        mask_b.set_bit(1, true);

        mask_a.nand(&mask_b);

        assert!(!mask_a.bit(0).unwrap());
        assert!(!mask_a.bit(1).unwrap());
        assert!(mask_a.bit(2).unwrap());
        assert!(!mask_a.bit(3).unwrap());
    }

    #[test]
    fn or() {
        let mut mask_a = DiffMask::new(1);
        mask_a.set_bit(1, true);
        mask_a.set_bit(2, true);

        let mut mask_b = DiffMask::new(1);
        mask_b.set_bit(2, true);
        mask_b.set_bit(3, true);

        mask_a.or(&mask_b);

        assert!(!mask_a.bit(0).unwrap());
        assert!(mask_a.bit(1).unwrap());
        assert!(mask_a.bit(2).unwrap());
        assert!(mask_a.bit(3).unwrap());
        assert!(!mask_a.bit(4).unwrap());
    }

    #[test]
    fn clone() {
        let mut mask_a = DiffMask::new(1);
        mask_a.set_bit(1, true);
        mask_a.set_bit(4, true);

        let mask_b = mask_a.clone();

        assert!(mask_b.bit(1).unwrap());
        assert!(!mask_b.bit(3).unwrap());
        assert!(mask_b.bit(4).unwrap());
    }
}

#[cfg(test)]
mod double_byte_tests {
    use crate::shared::DiffMask;

    #[test]
    fn getset() {
        let mut mask = DiffMask::new(2);

        mask.set_bit(0, true);
        mask.set_bit(4, true);
        mask.set_bit(8, true);
        mask.set_bit(12, true);
        mask.set_bit(8, false);

        assert!(mask.bit(0).unwrap());
        assert!(mask.bit(4).unwrap());
        assert!(!mask.bit(8).unwrap());
        assert!(mask.bit(12).unwrap());
        assert!(!mask.bit(13).unwrap());
    }

    #[test]
    fn clear() {
        let mut mask = DiffMask::new(2);

        mask.set_bit(0, true);
        mask.set_bit(4, true);
        mask.set_bit(8, true);
        mask.set_bit(12, true);

        mask.clear();

        assert!(!mask.bit(0).unwrap());
        assert!(!mask.bit(4).unwrap());
        assert!(!mask.bit(8).unwrap());
        assert!(!mask.bit(12).unwrap());
    }

    #[test]
    fn is_clear_true() {
        let mut mask = DiffMask::new(2);

        mask.set_bit(9, true);

        assert!(!mask.is_clear());

        mask.set_bit(9, false);

        assert!(mask.is_clear());
    }

    #[test]
    fn bytes() {
        let mask = DiffMask::new(2);
        assert!(mask.byte_number() == 2);
    }

    #[test]
    fn byte() {
        let mut mask = DiffMask::new(2);
        mask.set_bit(10, true);
        let byte = mask.byte(1);
        assert!(byte == 4);
    }

    #[test]
    fn nand() {
        let mut mask_a = DiffMask::new(2);
        mask_a.set_bit(1, true);
        mask_a.set_bit(2, true);
        mask_a.set_bit(9, true);
        mask_a.set_bit(10, true);

        let mut mask_b = DiffMask::new(2);
        mask_b.set_bit(1, true);
        mask_b.set_bit(9, true);

        mask_a.nand(&mask_b);

        assert!(!mask_a.bit(0).unwrap());
        assert!(!mask_a.bit(1).unwrap());
        assert!(mask_a.bit(2).unwrap());
        assert!(!mask_a.bit(3).unwrap());

        assert!(!mask_a.bit(8).unwrap());
        assert!(!mask_a.bit(9).unwrap());
        assert!(mask_a.bit(10).unwrap());
        assert!(!mask_a.bit(11).unwrap());
    }

    #[test]
    fn or() {
        let mut mask_a = DiffMask::new(2);
        mask_a.set_bit(4, true);
        mask_a.set_bit(8, true);

        let mut mask_b = DiffMask::new(2);
        mask_b.set_bit(8, true);
        mask_b.set_bit(12, true);

        mask_a.or(&mask_b);

        assert!(!mask_a.bit(0).unwrap());
        assert!(mask_a.bit(4).unwrap());
        assert!(mask_a.bit(8).unwrap());
        assert!(mask_a.bit(12).unwrap());
        assert!(!mask_a.bit(15).unwrap());
    }

    #[test]
    fn clone() {
        let mut mask_a = DiffMask::new(2);
        mask_a.set_bit(2, true);
        mask_a.set_bit(10, true);

        let mask_b = mask_a.clone();

        assert!(mask_b.bit(2).unwrap());
        assert!(!mask_b.bit(4).unwrap());
        assert!(!mask_b.bit(9).unwrap());
        assert!(mask_b.bit(10).unwrap());
    }
}
