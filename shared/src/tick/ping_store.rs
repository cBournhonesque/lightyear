use crate::tick::time::WrappedTime;
use crate::utils::sequence_buffer::SequenceBuffer;
use crate::utils::wrapping_id;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::collections::HashMap;

wrapping_id!(PingId);

const PING_BUFFER_SIZE: usize = 32;

/// Data structure to store the latest pings sent to remote
pub struct PingStore {
    latest_ping_id: PingId,
    buffer: SequenceBuffer<PingId, WrappedTime, PING_BUFFER_SIZE>,
}

impl PingStore {
    pub fn new() -> Self {
        PingStore {
            latest_ping_id: PingId(0),
            buffer: SequenceBuffer::new(),
        }
    }

    /// Pushes a new ping into the store and returns the corresponding ping id
    pub fn push_new(&mut self, now: WrappedTime) -> PingId {
        // save current ping index and add a new ping instant associated with it
        let ping_id = self.latest_ping_id;
        self.latest_ping_id += 1;
        self.buffer.push(&ping_id, now);
        ping_id
    }

    pub fn remove(&mut self, ping_id: PingId) -> Option<WrappedTime> {
        self.buffer.remove(&ping_id)
    }
}
