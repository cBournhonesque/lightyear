use crate::packet::message::SingleData;
use crate::packet::packet::{FragmentData, FRAGMENT_SIZE};
use crate::packet::wrapping_id::MessageId;
use crate::{BitSerializable, MessageContainer, ReadBuffer, ReadWordBuffer};
use anyhow::Result;
use bytes::Bytes;
use std::collections::HashMap;
use tracing::trace;

/// `FragmentReceiver` is used to reconstruct fragmented messages
pub(crate) struct FragmentSender {
    pub(crate) fragment_size: usize,
}

impl FragmentSender {
    pub fn new() -> Self {
        Self {
            // TODO: make this overridable?
            fragment_size: FRAGMENT_SIZE,
        }
    }
    pub fn build_fragments(
        &mut self,
        fragment_message_id: MessageId,
        fragment_bytes: Bytes,
    ) -> Vec<FragmentData> {
        let chunks = fragment_bytes.chunks(self.fragment_size);
        let num_fragments = chunks.len();
        chunks
            .enumerate()
            // TODO: ideally we don't clone here but we take ownership of the output of writer
            .map(|(fragment_index, chunk)| FragmentData {
                message_id: fragment_message_id,
                fragment_id: fragment_index as u8,
                num_fragments: num_fragments as u8,
                bytes: fragment_bytes.slice_ref(chunk),
            })
            .collect::<_>()
    }
}
