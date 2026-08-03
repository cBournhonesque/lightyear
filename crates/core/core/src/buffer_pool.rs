//! Bounded recycling for split [`BytesMut`] allocations.

use alloc::vec::Vec;
use bytes::BytesMut;

/// A bounded pool of fixed-capacity [`BytesMut`] allocations.
///
/// The pool supports zero-copy ownership handoffs through
/// [`split_for_handoff`](Self::split_for_handoff). That operation returns the initialized prefix
/// and retains the empty sibling tail. Both views share one allocation, so the tail remains
/// pending until every handed-off view has been dropped and [`BytesMut::try_reclaim`] can recover
/// the complete allocation.
///
/// Ready and pending buffers are kept separate so [`take`](Self::take) is O(1). Call
/// [`reclaim_pending`](Self::reclaim_pending) once at the start of a processing pass instead of
/// probing every pending allocation for every buffer request.
///
/// The returned prefix remains a [`BytesMut`]. Callers that need immutable shared views can freeze
/// it after the handoff; callers that need in-place mutation can keep it mutable.
#[derive(Debug)]
pub struct BufferPool {
    buffer_capacity: usize,
    max_retained: usize,
    ready: Vec<BytesMut>,
    pending: Vec<BytesMut>,
    misses: usize,
}

impl BufferPool {
    /// Creates an empty pool for allocations with exactly `buffer_capacity` visible capacity.
    ///
    /// At most `max_retained` allocations are held across the ready and pending partitions.
    pub const fn new(buffer_capacity: usize, max_retained: usize) -> Self {
        Self {
            buffer_capacity,
            max_retained,
            ready: Vec::new(),
            pending: Vec::new(),
            misses: 0,
        }
    }

    /// Allocates up to `count` immediately writable buffers without recording pool misses.
    ///
    /// This is intended for setup-time preallocation. Existing retained buffers count toward both
    /// `count` and the pool's retention bound.
    pub fn preallocate(&mut self, count: usize) {
        let target = count.min(self.max_retained);
        while self.retained_len() < target {
            self.ready
                .push(BytesMut::with_capacity(self.buffer_capacity));
        }
    }

    /// Returns an empty writable buffer with at least the configured capacity.
    ///
    /// A miss allocates a new buffer. That allocation can later enter the pool through
    /// [`recycle`](Self::recycle) or [`split_for_handoff`](Self::split_for_handoff).
    pub fn take(&mut self) -> BytesMut {
        if let Some(mut buffer) = self.ready.pop() {
            debug_assert!(buffer.capacity() >= self.buffer_capacity);
            buffer.clear();
            return buffer;
        }
        self.misses += 1;
        BytesMut::with_capacity(self.buffer_capacity)
    }

    /// Updates the required capacity, dropping all retained allocations when it changes.
    ///
    /// Existing allocations cannot safely be reclassified because the pool intentionally retains
    /// only allocations whose complete visible capacity matched the previous value exactly.
    pub fn set_buffer_capacity(&mut self, buffer_capacity: usize) {
        if self.buffer_capacity != buffer_capacity {
            self.buffer_capacity = buffer_capacity;
            self.ready.clear();
            self.pending.clear();
        }
    }

    /// Promotes pending tails whose handed-off siblings have all been dropped.
    pub fn reclaim_pending(&mut self) {
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].try_reclaim(self.buffer_capacity) {
                let buffer = self.pending.swap_remove(index);
                self.ready.push(buffer);
            } else {
                index += 1;
            }
        }
    }

    /// Recycles a complete buffer that has not crossed an ownership boundary.
    ///
    /// Buffers whose visible capacity differs from the configured capacity are dropped. This keeps
    /// unexpectedly enlarged allocations from weakening the pool's memory bound.
    pub fn recycle(&mut self, mut buffer: BytesMut) {
        buffer.clear();
        if buffer.capacity() != self.buffer_capacity {
            return;
        }
        self.enqueue(buffer);
    }

    /// Splits a buffer for zero-copy handoff and retains its empty sibling tail when eligible.
    ///
    /// Eligibility is checked before splitting because the tail's visible capacity does not reveal
    /// the complete allocation size. The returned prefix is mutable; callers may subsequently call
    /// [`BytesMut::freeze`] without copying.
    pub fn split_for_handoff(&mut self, mut buffer: BytesMut) -> BytesMut {
        let retain_tail = buffer.capacity() == self.buffer_capacity;
        let payload = buffer.split();
        if retain_tail {
            self.enqueue(buffer);
        }
        payload
    }

    /// Returns how many calls to [`take`](Self::take) allocated because no ready buffer existed.
    pub const fn misses(&self) -> usize {
        self.misses
    }

    fn enqueue(&mut self, mut buffer: BytesMut) {
        if self.retained_len() >= self.max_retained {
            return;
        }
        if buffer.try_reclaim(self.buffer_capacity) {
            self.ready.push(buffer);
        } else {
            self.pending.push(buffer);
        }
    }

    fn retained_len(&self) -> usize {
        self.ready.len() + self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPACITY: usize = 128;

    #[test]
    fn split_tail_becomes_ready_after_mutable_payload_drop() {
        let mut pool = BufferPool::new(CAPACITY, 4);
        let mut buffer = pool.take();
        buffer.extend_from_slice(b"payload");

        let payload = pool.split_for_handoff(buffer);
        assert!(pool.ready.is_empty());
        assert_eq!(pool.pending.len(), 1);

        pool.reclaim_pending();
        assert!(pool.ready.is_empty());

        drop(payload);
        pool.reclaim_pending();
        assert_eq!(pool.ready.len(), 1);
        assert!(pool.pending.is_empty());
    }

    #[test]
    fn split_tail_becomes_ready_after_frozen_payload_drop() {
        let mut pool = BufferPool::new(CAPACITY, 4);
        let mut buffer = pool.take();
        buffer.extend_from_slice(b"payload");

        let payload = pool.split_for_handoff(buffer).freeze();
        assert_eq!(pool.pending.len(), 1);

        drop(payload);
        pool.reclaim_pending();
        assert_eq!(pool.ready.len(), 1);
        assert!(pool.pending.is_empty());
    }

    #[test]
    fn oversized_complete_buffer_is_not_retained() {
        let mut pool = BufferPool::new(CAPACITY, 4);
        pool.recycle(BytesMut::with_capacity(CAPACITY * 2));

        assert_eq!(pool.retained_len(), 0);
        let misses = pool.misses();
        let _ = pool.take();
        assert_eq!(pool.misses(), misses + 1);
    }

    #[test]
    fn preallocation_and_retention_are_bounded() {
        let mut pool = BufferPool::new(CAPACITY, 2);
        pool.preallocate(usize::MAX);
        assert_eq!(pool.ready.len(), 2);
        assert_eq!(pool.misses(), 0);

        let first = pool.take();
        let second = pool.take();
        let third = pool.take();
        assert_eq!(pool.misses(), 1);

        pool.recycle(first);
        pool.recycle(second);
        pool.recycle(third);
        assert_eq!(pool.retained_len(), 2);
    }

    #[test]
    fn changing_capacity_drops_retained_buffers() {
        let mut pool = BufferPool::new(CAPACITY, 2);
        pool.preallocate(1);

        pool.set_buffer_capacity(CAPACITY * 2);

        assert_eq!(pool.retained_len(), 0);
        assert_eq!(pool.take().capacity(), CAPACITY * 2);
    }
}
