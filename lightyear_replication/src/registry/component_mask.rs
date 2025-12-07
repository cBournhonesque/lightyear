use core::ops::BitOrAssign;

use smallbitvec::SmallBitVec;

/// Wraps a bitvec to provide a dynamically growing bitmask for compactly storing component IDs.
#[derive(Default, Debug, Clone)]
pub struct ComponentMask {
    /// Each bit corresponds to a [`usize`].
    bits: SmallBitVec,
}

impl ComponentMask {
    pub(crate) fn contains(&self, index: usize) -> bool {
        self.bits.get(index).unwrap_or(false)
    }

    pub(crate) fn insert(&mut self, index: usize) {
        if index >= self.bits.len() {
            self.bits.resize(index + 1, false);
        }
        self.bits.set(index, true);
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.bits.set(index, false);
    }

    pub(crate) fn is_heap(&self) -> bool {
        self.bits.heap_ptr().is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.bits.clear();
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> {
        self.bits
            .iter()
            .enumerate()
            .filter_map(|(index, value)| value.then_some(index))
    }
}

impl BitOrAssign<&ComponentMask> for ComponentMask {
    #[inline]
    fn bitor_assign(&mut self, rhs: &ComponentMask) {
        if self.bits.len() < rhs.bits.len() {
            self.bits.resize(rhs.bits.len(), false);
        }

        for index in 0..self.bits.len().min(rhs.bits.len()) {
            // SAFETY: index is correct.
            unsafe {
                let value = self.bits.get_unchecked(index) | rhs.bits.get_unchecked(index);
                self.bits.set_unchecked(index, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use smallbitvec::sbvec;
    use test_log::test;

    use super::*;

    #[test]
    fn insert_remove() {
        let mut mask = ComponentMask {
            bits: sbvec![false; 3],
        };

        mask.insert(0);
        mask.insert(2);
        mask.insert(10);

        assert!(mask.contains(0));
        assert!(!mask.contains(1));
        assert!(mask.contains(2));
        assert!(mask.contains(10));
        assert!(!mask.contains(100));

        mask.remove(2);
        assert!(!mask.contains(2));
    }

    #[test]
    fn bitor_assign() {
        let mut a = ComponentMask {
            bits: sbvec![true, false, true],
        };
        let b = ComponentMask {
            bits: sbvec![false, true, false, true],
        };

        a |= &b;

        assert_eq!(a.bits, sbvec![true; 4]);
    }
}
