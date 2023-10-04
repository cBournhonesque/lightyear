const NETCODE_REPLAY_BUFFER_SIZE: usize = 256;
const EMPTY: u64 = u64::MAX;

#[derive(Debug, Clone)]
pub struct ReplayProtection {
    most_recent_sequence: u64,
    received_packet: [u64; NETCODE_REPLAY_BUFFER_SIZE],
}

impl Default for ReplayProtection {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayProtection {
    pub fn new() -> Self {
        Self {
            most_recent_sequence: 0,
            received_packet: [EMPTY; NETCODE_REPLAY_BUFFER_SIZE],
        }
    }

    pub fn already_received(&self, sequence: u64) -> bool {
        if sequence + NETCODE_REPLAY_BUFFER_SIZE as u64 <= self.most_recent_sequence {
            return true;
        }

        let index = sequence as usize % NETCODE_REPLAY_BUFFER_SIZE;
        if self.received_packet[index] == EMPTY {
            return false;
        }

        if self.received_packet[index] >= sequence {
            return true;
        }

        false
    }

    pub fn advance_sequence(&mut self, sequence: u64) {
        if sequence > self.most_recent_sequence {
            self.most_recent_sequence = sequence;
        }

        let index = sequence as usize % NETCODE_REPLAY_BUFFER_SIZE;
        self.received_packet[index] = sequence;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_protection() {
        let mut replay_protection = ReplayProtection::new();
        assert_eq!(replay_protection.most_recent_sequence, 0);

        // New packets aren't already received
        let max_sequence = (NETCODE_REPLAY_BUFFER_SIZE * 4) as u64;
        for i in 0..max_sequence {
            assert!(!replay_protection.already_received(i));
            replay_protection.advance_sequence(i);
        }

        // Old packes outside the buffer should be considered already received
        assert!(replay_protection.already_received(0));

        // Packets received a second time should be already received
        for i in max_sequence - 10..max_sequence {
            assert!(replay_protection.already_received(i));
        }

        // Jumping to a higher sequence should be considered not already received
        assert!(!replay_protection.already_received(max_sequence + NETCODE_REPLAY_BUFFER_SIZE as u64));

        // Old packets should be considered received
        for i in 0..max_sequence {
            assert!(replay_protection.already_received(i));
        }
    }
}
