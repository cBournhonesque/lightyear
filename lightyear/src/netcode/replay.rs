const REPLAY_PROTECTION_BUFFER_SIZE: usize = 256;
const UNRECEIVED: u64 = u64::MAX;

#[derive(Clone)]
pub struct ReplayProtection {
    most_recent_sequence: u64,
    received_packet: [u64; REPLAY_PROTECTION_BUFFER_SIZE],
}

impl ReplayProtection {
    pub fn new() -> Self {
        Self {
            most_recent_sequence: 0,
            received_packet: [UNRECEIVED; REPLAY_PROTECTION_BUFFER_SIZE],
        }
    }
    pub fn advance_sequence(&mut self, sequence: u64) {
        if sequence > self.most_recent_sequence {
            self.most_recent_sequence = sequence;
        }

        let index = sequence as usize % self.received_packet.len();

        self.received_packet[index] = sequence;
    }

    pub fn is_already_received(&self, sequence: u64) -> bool {
        if sequence + self.received_packet.len() as u64 <= self.most_recent_sequence {
            return true;
        }

        let index = sequence as usize % self.received_packet.len();

        if self.received_packet[index] == UNRECEIVED {
            return false;
        }

        self.received_packet[index] >= sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_protection() {
        let mut replay_protection = ReplayProtection::new();

        // Nothing received yet
        assert!(!replay_protection.is_already_received(0));

        // Send a bunch of packets
        for i in 0..REPLAY_PROTECTION_BUFFER_SIZE * 2 {
            replay_protection.advance_sequence(i as u64);
        }

        // Check that they were all received
        for i in 0..REPLAY_PROTECTION_BUFFER_SIZE * 2 {
            assert!(replay_protection.is_already_received(i as u64));
        }

        // Make sure a future packet is not received
        assert!(
            !replay_protection.is_already_received((REPLAY_PROTECTION_BUFFER_SIZE * 2 + 1) as u64)
        );

        // Check that the last packet was the most recent
        assert_eq!(
            replay_protection.most_recent_sequence,
            (REPLAY_PROTECTION_BUFFER_SIZE * 2 - 1) as u64
        );
    }
}
