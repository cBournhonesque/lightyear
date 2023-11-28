use std::{cmp::Ordering, collections::BinaryHeap};

/// A buffer that contains items associated with a key (a Tick, Instant, etc.)
///
/// Elements in the buffer are popped only when they are 'ready', i.e.
/// when the key associated with the item is less than or equal to the current key
///
/// The most recent item (by associated key) is returned first
#[derive(Clone, Default, Debug)]
pub struct ReadyBuffer<K: Ord, T: PartialEq> {
    // TODO: compare performance with a SequenceBuffer of fixed size
    // TODO: add a maximum size to the buffer. The elements that are farther away from being ready dont' get added?
    /// min heap: we pop the items with smallest key first
    pub heap: BinaryHeap<ItemWithReadyKey<K, T>>,
}

impl<K: Ord, T: PartialEq> ReadyBuffer<K, T> {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::default(),
        }
    }
}

impl<K: Ord, T: PartialEq> ReadyBuffer<K, T> {
    /// Adds an item to the heap marked by time
    pub fn add_item(&mut self, key: K, item: T) {
        self.heap.push(ItemWithReadyKey { key, item });
    }

    /// Returns whether or not there is an item that is ready to be returned
    /// (i.e. we are beyond the instant associated with the item)
    pub fn has_item(&self, current_key: &K) -> bool {
        // if self.heap.is_empty() {
        //     return false;
        // }
        if let Some(item) = self.heap.peek() {
            // if the current_key is bigger than the item key, we are ready to return
            let cmp = item.key.cmp(current_key);
            return matches!(cmp, Ordering::Less | Ordering::Equal);
        }
        false
    }

    /// Pops the top item (with smallest key) from the queue if the key is above the provided `current_key`
    /// (i.e. we are beyond the instant associated with the item)
    pub fn pop_item(&mut self, current_key: &K) -> Option<(K, T)> {
        if self.has_item(current_key) {
            if let Some(container) = self.heap.pop() {
                return Some((container.key, container.item));
            }
        }
        None
    }

    /// Pop all items that are older than the provided key, then return the value for the most recent item
    /// with a key older or equal to the provided key
    /// (i.e. if we have keys 1, 4, 6, pop_until(5) will pop 1, 4 and return the value for key 4)
    /// /// (i.e. if we have keys 1, 4, 6, pop_until(4) will pop 1, 4 and return the value for key 4)
    pub(crate) fn pop_until(&mut self, key: &K) -> Option<(K, T)> {
        if self.heap.is_empty() {
            return None;
        }
        let mut val = None;
        loop {
            if let Some(item_with_key) = self.heap.peek() {
                // we have a new update that is older than what we want, stop
                if item_with_key.key > *key {
                    // put back the update in the heap
                    // self.heap.push(item_with_key);
                    break;
                }
            } else {
                // heap is empty
                break;
            }
            // safety: we know that the heap is not empty and that the key is <= the provided key
            val = self.heap.pop().map(|item| (item.key, item.item));
        }
        val
    }

    /// Returns the length of the underlying queue
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Checks if the underlying queue is empty
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct ItemWithReadyKey<K: Ord, T> {
    pub key: K,
    pub item: T,
}

impl<K: Ord, T: PartialEq> Eq for ItemWithReadyKey<K, T> {}

impl<K: Ord, T: PartialEq> PartialEq<Self> for ItemWithReadyKey<K, T> {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item && self.key == other.key
    }
}

impl<K: Ord, T: PartialEq> PartialOrd<Self> for ItemWithReadyKey<K, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// BinaryHeap is a max-heap, so we must reverse the ordering of the Instants
/// to get a min-heap
impl<K: Ord, T: PartialEq> Ord for ItemWithReadyKey<K, T> {
    fn cmp(&self, other: &ItemWithReadyKey<K, T>) -> Ordering {
        other.key.cmp(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mock_instant::Instant;
    use mock_instant::MockClock;

    use crate::tick::Tick;

    use super::*;

    #[test]
    fn test_time_heap() {
        let mut heap = ReadyBuffer::<Instant, u64>::new();
        let now = Instant::now();

        // can insert items in any order of time
        heap.add_item(now + Duration::from_secs(2), 2);
        heap.add_item(now + Duration::from_secs(1), 1);
        heap.add_item(now + Duration::from_secs(3), 3);

        // no items are visible
        assert_eq!(heap.has_item(&Instant::now()), false);

        // we move the clock to 2, 2 items should be visible, in order of insertion
        MockClock::advance(Duration::from_secs(2));
        matches!(heap.pop_item(&Instant::now()), Some((_, 1)));
        matches!(heap.pop_item(&Instant::now()), Some((_, 2)));
        assert_eq!(heap.pop_item(&Instant::now()), None);
        assert_eq!(heap.len(), 1);
    }

    #[test]
    fn test_pop_until() {
        let mut buffer = ReadyBuffer::new();

        // check when we try to access a value when the buffer is empty
        assert_eq!(buffer.pop_until(&Tick(0)), None);

        // check when we try to access an exact tick
        buffer.add_item(Tick(1), 1);
        buffer.add_item(Tick(2), 2);
        assert_eq!(buffer.pop_until(&Tick(2)), Some((Tick(2), 2)));
        // check that we cleared older ticks
        assert!(buffer.is_empty());

        // check when we try to access a value in-between ticks
        buffer.add_item(Tick(1), 1);
        buffer.add_item(Tick(3), 3);
        assert_eq!(buffer.pop_until(&Tick(2)), Some((Tick(1), 1)));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.pop_until(&Tick(4)), Some((Tick(3), 3)));
        assert!(buffer.is_empty());

        // check when we try to access a value before any ticks
        buffer.add_item(Tick(1), 1);
        assert_eq!(buffer.pop_until(&Tick(0)), None);
        assert_eq!(buffer.len(), 1);
    }
}
