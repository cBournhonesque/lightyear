#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::collections::HashMap;
use tracing::{error, trace};

use crate::packet::message::{FragmentIndex, MessageId};
use crate::shared::time_manager::WrappedTime;

/// `FragmentReceiver` is used to reconstruct fragmented messages
#[derive(Debug, PartialEq)]
pub struct FragmentAckReceiver {
    fragment_messages: HashMap<MessageId, FragmentAckTracker>,
}

impl FragmentAckReceiver {
    pub fn new() -> Self {
        Self {
            fragment_messages: HashMap::default(),
        }
    }

    pub fn add_new_fragment_to_wait_for(&mut self, message_id: MessageId, num_fragments: usize) {
        self.fragment_messages
            .entry(message_id)
            .or_insert_with(|| FragmentAckTracker::new(num_fragments));
    }

    /// Discard all messages for which the latest ack was received before the cleanup time
    /// (i.e. we probably lost some fragments and we will never get all the acks for this fragmented message)
    ///
    /// If we don't keep track of the last received time, we will never clean up the messages.
    pub fn cleanup(&mut self, cleanup_time: WrappedTime) {
        self.fragment_messages.retain(|_, c| {
            c.last_received
                .map(|t| t > cleanup_time)
                .unwrap_or_else(|| true)
        })
    }

    /// We receive a fragment ack, and return true if the entire fragment was acked.
    pub fn receive_fragment_ack(
        &mut self,
        message_id: MessageId,
        fragment_index: FragmentIndex,
        current_time: Option<WrappedTime>,
    ) -> bool {
        let Some(fragment_ack_tracker) = self.fragment_messages.get_mut(&message_id) else {
            error!("Received fragment ack for unknown message id");
            return false;
        };

        // completed the fragmented message!
        if fragment_ack_tracker.receive_ack(fragment_index as usize, current_time) {
            self.fragment_messages.remove(&message_id);
            return true;
        }

        false
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Data structure to keep track of when an entire fragment message is acked
pub struct FragmentAckTracker {
    num_fragments: usize,
    num_received_fragments: usize,
    received: Vec<bool>,
    last_received: Option<WrappedTime>,
}

impl FragmentAckTracker {
    pub fn new(num_fragments: usize) -> Self {
        Self {
            num_fragments,
            num_received_fragments: 0,
            received: vec![false; num_fragments],
            last_received: None,
        }
    }

    /// Receive a fragment index ack, and return true if the entire fragment was acked.
    pub fn receive_ack(
        &mut self,
        fragment_index: usize,
        received_time: Option<WrappedTime>,
    ) -> bool {
        self.last_received = received_time;

        if !self.received[fragment_index] {
            self.received[fragment_index] = true;
            self.num_received_fragments += 1;
        }

        if self.num_received_fragments == self.num_fragments {
            trace!("Received all fragments ack!");
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_receive_fragments() {
        let mut receiver = FragmentAckReceiver::new();

        receiver.add_new_fragment_to_wait_for(MessageId(0), 2);

        assert!(!receiver.receive_fragment_ack(MessageId(0), 0, None));
        // receiving the same fragment twice should do nothing
        assert!(!receiver.receive_fragment_ack(MessageId(0), 0, None));
        // we receive the entire fragment: should return true
        assert!(receiver.receive_fragment_ack(MessageId(0), 1, None));

        assert!(receiver.fragment_messages.is_empty());
    }

    #[test]
    fn test_cleanup() {
        let mut receiver = FragmentAckReceiver::new();

        receiver.add_new_fragment_to_wait_for(MessageId(0), 2);

        assert!(!receiver.receive_fragment_ack(MessageId(0), 0, Some(WrappedTime::new(150))));
        receiver.cleanup(WrappedTime::new(170));
        assert!(receiver.fragment_messages.is_empty());
    }
}
