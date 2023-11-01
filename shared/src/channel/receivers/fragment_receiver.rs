use crate::packet::message::{FragmentData, SingleData};
use crate::packet::packet::FRAGMENT_SIZE;
use crate::packet::wrapping_id::MessageId;
use crate::{BitSerializable, MessageContainer, ReadBuffer, ReadWordBuffer};
use anyhow::Result;
use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use tracing::trace;

/// `FragmentReceiver` is used to reconstruct fragmented messages
pub struct FragmentReceiver {
    fragment_messages: HashMap<MessageId, FragmentConstructor>,
}

impl FragmentReceiver {
    pub fn new() -> Self {
        Self {
            fragment_messages: HashMap::new(),
        }
    }

    pub fn receive_fragment(&mut self, fragment: FragmentData) -> Result<Option<SingleData>> {
        let fragment_message = self
            .fragment_messages
            .entry(fragment.message_id)
            .or_insert_with(|| FragmentConstructor::new(fragment.num_fragments as usize));

        // completed the fragmented message!
        if let Some(payload) = fragment_message
            .receive_fragment(fragment.fragment_id as usize, fragment.bytes.as_ref())?
        {
            self.fragment_messages.remove(&fragment.message_id);
            return Ok(Some(SingleData::new(Some(fragment.message_id), payload)));
        }

        Ok(None)
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
}

impl FragmentConstructor {
    pub fn new(num_fragments: usize) -> Self {
        Self {
            num_fragments,
            num_received_fragments: 0,
            received: vec![false; num_fragments],
            bytes: vec![0; num_fragments * FRAGMENT_SIZE],
        }
    }

    pub fn receive_fragment(
        &mut self,
        fragment_index: usize,
        bytes: &[u8],
    ) -> Result<Option<Bytes>> {
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
            let payload = std::mem::take(&mut self.bytes);
            return Ok(Some(payload.into()));
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::senders::fragment_sender::FragmentSender;

    #[test]
    fn test_receiver() -> Result<()> {
        let mut receiver = FragmentReceiver::new();
        let num_bytes = (FRAGMENT_SIZE as f32 * 1.5) as usize;
        let message_bytes = Bytes::from(vec![1 as u8; num_bytes]);
        let fragments = FragmentSender::new().build_fragments(MessageId(0), message_bytes.clone());

        assert_eq!(receiver.receive_fragment(fragments[0].clone())?, None);
        assert_eq!(
            receiver.receive_fragment(fragments[1].clone())?,
            Some(SingleData {
                id: Some(MessageId(0)),
                bytes: message_bytes.clone()
            })
        );
        Ok(())
    }
}
