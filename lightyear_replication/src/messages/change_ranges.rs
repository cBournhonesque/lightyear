use alloc::vec::Vec;
use core::ops::Range;

use lightyear_serde::varint::varint_len;

/// Component insertions or mutations for an entity in form of serialized ranges
/// from [`SerializedData`](super::serialized_data::SerializedData).
///
/// Used inside [`Updates`](super::actions::Updates) and
/// [`Mutations`](super::updates::Mutations).
#[derive(Debug)]
pub(super) struct ChangeRanges {
    pub(super) entity: Range<usize>,
    pub(super) components_len: usize,
    pub(super) components: Vec<Range<usize>>,
}

impl ChangeRanges {
    /// Returns serialized size.
    pub(super) fn size(&self) -> usize {
        let len_size = varint_len(self.components_len as u64);
        self.entity.len() + len_size + self.components_size()
    }

    /// Like [`Self::size`], but uses components size instead of components count.
    ///
    /// It usually costs more bytes (because the number is bigger),
    /// but allows to skip data on deserialization.
    pub(super) fn size_with_components_size(&self) -> usize {
        let components_size = self.components_size();
        let len_size = varint_len(components_size as u64);
        self.entity.len() + len_size + components_size
    }

    pub(super) fn components_size(&self) -> usize {
        self.components.iter().map(|range| range.len()).sum()
    }

    pub(super) fn add_component(&mut self, component: Range<usize>) {
        self.components_len += 1;

        if let Some(last) = self.components.last_mut() {
            // Append to previous range if possible.
            if last.end == component.start {
                last.end = component.end;
                return;
            }
        }

        self.components.push(component);
    }

    pub(super) fn extend(&mut self, other: &Self) {
        self.components.extend(other.components.iter().cloned());
        self.components_len += other.components_len;
    }
}
