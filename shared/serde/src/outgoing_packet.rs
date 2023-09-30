use crate::MTU_SIZE_BYTES;

pub struct OutgoingPacket {
    payload_length: usize,
    payload: [u8; MTU_SIZE_BYTES],
}

impl OutgoingPacket {
    pub fn new(payload_length: usize, payload: [u8; MTU_SIZE_BYTES]) -> Self {
        Self {
            payload_length,
            payload,
        }
    }

    pub fn slice(&self) -> &[u8] {
        &self.payload[0..self.payload_length]
    }
}
