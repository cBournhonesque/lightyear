use crate::packet::packet::{FragmentData, FRAGMENT_SIZE};
use crate::packet::wrapping_id::MessageId;
use crate::{BitSerializable, MessageContainer, ReadBuffer, ReadWordBuffer};
use anyhow::Result;
use bytes::Bytes;
use std::collections::HashMap;
use tracing::trace;

pub struct FragmentReceiver {
    fragment_messages: HashMap<MessageId, FragmentConstructor>,
}

impl FragmentReceiver {
    pub fn new() -> Self {
        Self {
            fragment_messages: HashMap::new(),
        }
    }

    pub fn receive_fragment<M: BitSerializable>(
        &mut self,
        message_id: MessageId,
        fragment_data: FragmentData,
    ) -> Result<Option<MessageContainer<M>>> {
        let fragment_message = self
            .fragment_messages
            .entry(message_id)
            .or_insert_with(|| FragmentConstructor::new(fragment_data.num_fragments));

        // completed the fragmented message!
        if let Some(payload) = fragment_message
            .receive_fragment(fragment_data.fragment_id, fragment_data.bytes.as_ref())?
        {
            self.fragment_messages.remove(&message_id);

            let mut reader = ReadWordBuffer::start_read(&payload);
            let message = reader.deserialize::<M>()?;
            let mut message_container = MessageContainer::new(message);
            message_container.set_id(message_id);
            // reader.finish_read()
            return Ok(Some(message_container));
        }

        Ok(None)
    }
}

#[derive(Debug, Clone)]
/// Data structure to reconstruct a single fragment from individual parts
pub struct FragmentConstructor {
    num_fragments: u8,
    num_received_fragments: u8,
    received: Vec<bool>,
    bytes: Vec<u8>,
}

impl FragmentConstructor {
    pub fn new(num_fragments: u8) -> Self {
        Self {
            num_fragments,
            num_received_fragments: 0,
            received: vec![false; num_fragments as usize],
            bytes: vec![0; num_fragments as usize * FRAGMENT_SIZE],
        }
    }

    pub fn receive_fragment(&mut self, fragment_index: u8, bytes: &[u8]) -> Result<Option<Bytes>> {
        let is_last_fragment = fragment_index == self.num_fragments - 1;
        if !self.received[fragment_index] {
            self.received[fragment_index] = true;
            self.num_received_fragments += 1;

            if is_last_fragment {
                let len = (self.num_fragments as usize - 1) * FRAGMENT_SIZE + bytes.len();
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
