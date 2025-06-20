//! Store the latest pings sent to remote

use lightyear_core::time::Instant;
use lightyear_utils::sequence_buffer::SequenceBuffer;
use lightyear_utils::wrapping_id;

wrapping_id!(PingId);

const PING_BUFFER_SIZE: usize = 128;

/// Data structure to store the latest pings sent to remote
#[derive(Debug)]
pub struct PingStore {
    /// ID that will be assigned to the next ping sent
    latest_ping_id: PingId,
    /// Buffer storing the latest pings sent along with their associated time
    /// Older pings will get overwritten by newer pings
    buffer: SequenceBuffer<PingId, Instant, PING_BUFFER_SIZE>,
}

impl Default for PingStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PingStore {
    pub fn new() -> Self {
        PingStore {
            latest_ping_id: PingId(0),
            buffer: SequenceBuffer::new(),
        }
    }

    /// Pushes a new ping into the store and returns the corresponding ping id
    pub fn push_new(&mut self, now: Instant) -> PingId {
        // save current ping index and add a new ping instant associated with it
        let ping_id = self.latest_ping_id;
        self.latest_ping_id += 1;
        self.buffer.push(&ping_id, now);
        ping_id
    }

    /// Remove a ping from the store and returns the corresponding time if it exists
    pub fn remove(&mut self, ping_id: PingId) -> Option<Instant> {
        self.buffer.remove(&ping_id)
    }

    pub fn reset(&mut self) {
        self.latest_ping_id = PingId(0);
        self.buffer.clear();
    }
}
