#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::collections::HashMap;

use bytes::Bytes;
use tracing::trace;

use crate::packet::message::{FragmentData, MessageId};
use crate::packet::packet::FRAGMENT_SIZE;
use crate::prelude::Tick;
use crate::shared::time_manager::WrappedTime;

/// `FragmentReceiver` is used to reconstruct fragmented messages
#[derive(Debug)]
pub struct FragmentReceiver {
    fragment_messages: HashMap<MessageId, FragmentConstructor>,
}

impl FragmentReceiver {
    pub fn new() -> Self {
        Self {
            fragment_messages: HashMap::default(),
        }
    }

    /// Discard all messages for which the latest fragment was received before the cleanup time
    /// (i.e. we probably lost some fragments and we will never complete the message)
    ///
    /// If we don't keep track of the last received time, we will never clean up the messages.
    pub fn cleanup(&mut self, cleanup_time: WrappedTime) {
        self.fragment_messages.retain(|_, c| {
            c.last_received
                .map(|t| t > cleanup_time)
                .unwrap_or_else(|| true)
        })
    }

    /// Receive a fragment of a FragmentData message.
    ///
    /// When we complete the final message by aggregating all fragments, we will return the
    /// `remote_sent_tick` associated with the first fragment received.
    pub fn receive_fragment(
        &mut self,
        fragment: FragmentData,
        remote_sent_tick: Tick,
        current_time: Option<WrappedTime>,
    ) -> Option<(Tick, Bytes)> {
        let fragment_message = self
            .fragment_messages
            .entry(fragment.message_id)
            .or_insert_with(|| {
                FragmentConstructor::new(remote_sent_tick, fragment.num_fragments as usize)
            });

        // completed the fragmented message!
        if let Some(payload) = fragment_message.receive_fragment(
            fragment.fragment_id as usize,
            fragment.bytes.as_ref(),
            current_time,
        ) {
            self.fragment_messages.remove(&fragment.message_id);
            return Some(payload);
        }

        None
    }
}

#[derive(Debug, Clone)]
/// Data structure to reconstruct a single fragmented message from individual fragments
pub struct FragmentConstructor {
    num_fragments: usize,
    num_received_fragments: usize,
    received: Vec<bool>,
    // bytes: Bytes,
    bytes: Vec<u8>,

    tick: Tick,
    last_received: Option<WrappedTime>,
}

impl FragmentConstructor {
    pub fn new(tick: Tick, num_fragments: usize) -> Self {
        Self {
            num_fragments,
            num_received_fragments: 0,
            received: vec![false; num_fragments],
            bytes: vec![0; num_fragments * FRAGMENT_SIZE],
            tick,
            last_received: None,
        }
    }

    pub fn receive_fragment(
        &mut self,
        fragment_index: usize,
        bytes: &[u8],
        received_time: Option<WrappedTime>,
    ) -> Option<(Tick, Bytes)> {
        self.last_received = received_time;

        let is_last_fragment = fragment_index == self.num_fragments - 1;

        // TODO: check sizes?

        if !self.received[fragment_index] {
            self.received[fragment_index] = true;
            self.num_received_fragments += 1;

            if is_last_fragment {
                let len = (self.num_fragments - 1) * FRAGMENT_SIZE + bytes.len();
                self.bytes.resize(len, 0);
            }

            let start = fragment_index * FRAGMENT_SIZE;
            let end = start + bytes.len();
            self.bytes[start..end].copy_from_slice(bytes);
        }

        if self.num_received_fragments == self.num_fragments {
            trace!("Received all fragments!");
            let payload = core::mem::take(&mut self.bytes);
            return Some((self.tick, payload.into()));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::senders::fragment_sender::FragmentSender;

    use super::*;

    #[test]
    fn test_receiver() {
        let mut receiver = FragmentReceiver::new();
        let num_bytes = (FRAGMENT_SIZE as f32 * 1.5) as usize;
        let message_bytes = Bytes::from(vec![1u8; num_bytes]);
        let fragments = FragmentSender::new()
            .build_fragments(MessageId(0), None, message_bytes.clone())
            .unwrap();

        assert_eq!(
            receiver.receive_fragment(fragments[0].clone(), Tick(0), None),
            None
        );
        assert_eq!(
            receiver.receive_fragment(fragments[1].clone(), Tick(1), None),
            Some((Tick(0), message_bytes.clone()))
        );
    }
}
