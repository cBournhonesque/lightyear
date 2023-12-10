use std::collections::HashMap;

use anyhow::Result;
use bytes::Bytes;
use tracing::{error, trace};

use crate::packet::message::{FragmentData, FragmentIndex, MessageAck, MessageId, SingleData};
use crate::packet::packet::FRAGMENT_SIZE;
use crate::shared::time_manager::WrappedTime;

/// `FragmentReceiver` is used to reconstruct fragmented messages
pub struct FragmentAckReceiver {
    fragment_messages: HashMap<MessageId, FragmentAckTracker>,
}

impl FragmentAckReceiver {
    pub fn new() -> Self {
        Self {
            fragment_messages: HashMap::new(),
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
        let mut fragment_ack_tracker = self.fragment_messages.get(&message_id) else {
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

#[derive(Debug, Clone)]
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
    use crate::channel::senders::fragment_sender::FragmentSender;

    use super::*;

    #[test]
    fn test_receiver() -> Result<()> {
        let mut receiver = FragmentReceiver::new();
        let num_bytes = (FRAGMENT_SIZE as f32 * 1.5) as usize;
        let message_bytes = Bytes::from(vec![1u8; num_bytes]);
        let fragments =
            FragmentSender::new().build_fragments(MessageId(0), None, message_bytes.clone());

        assert_eq!(receiver.receive_fragment(fragments[0].clone(), None)?, None);
        assert_eq!(
            receiver.receive_fragment(fragments[1].clone(), None)?,
            Some(SingleData {
                id: Some(MessageId(0)),
                tick: None,
                bytes: message_bytes.clone(),
            })
        );
        Ok(())
    }
}
