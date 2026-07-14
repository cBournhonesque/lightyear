use crate::packet::compression::{
    CompressionConfig, PayloadCompressionCandidate, try_build_compressed_payload,
};
use crate::packet::message::{FragmentCompression, FragmentData, FragmentIndex, MessageId};
use crate::packet::packet::FRAGMENT_SIZE;
use alloc::vec::Vec;
use bytes::Bytes;
use tracing::trace;

/// `FragmentReceiver` is used to reconstruct fragmented messages
#[derive(Debug)]
pub(crate) struct FragmentSender {
    pub(crate) fragment_size: usize,
}

impl FragmentSender {
    pub fn new() -> Self {
        Self {
            fragment_size: FRAGMENT_SIZE,
        }
    }

    pub(crate) fn set_fragment_size(&mut self, fragment_size: usize) {
        debug_assert!(fragment_size > 0);
        self.fragment_size = fragment_size;
    }
    pub fn build_fragments(
        &self,
        fragment_message_id: MessageId,
        fragment_bytes: Bytes,
    ) -> Vec<FragmentData> {
        if fragment_bytes.len() <= self.fragment_size {
            unreachable!(
                "Message size must be at least {} to need to be fragmented",
                self.fragment_size
            );
        }
        self.build_fragments_with_compression(
            fragment_message_id,
            fragment_bytes,
            FragmentCompression::None,
        )
    }

    pub(crate) fn build_fragments_for_message(
        &self,
        fragment_message_id: MessageId,
        message: Bytes,
        compression: CompressionConfig,
    ) -> Vec<FragmentData> {
        if message.len() <= self.fragment_size {
            unreachable!(
                "Message size must be at least {} to need to be fragmented",
                self.fragment_size
            );
        }

        if let Ok(PayloadCompressionCandidate::Compressed {
            payload,
            original_len,
            compressed_len,
        }) = try_build_compressed_payload(message.as_ref(), compression)
            && let Some(fragment_compression) = fragment_compression(compression)
        {
            trace!(
                original_len,
                compressed_len, "compressed fragmented message payload"
            );
            return self.build_fragments_with_compression(
                fragment_message_id,
                Bytes::from(payload),
                fragment_compression,
            );
        }

        self.build_fragments(fragment_message_id, message)
    }

    fn build_fragments_with_compression(
        &self,
        fragment_message_id: MessageId,
        fragment_bytes: Bytes,
        compression: FragmentCompression,
    ) -> Vec<FragmentData> {
        let chunks = fragment_bytes.chunks(self.fragment_size);
        let num_fragments = chunks.len();
        chunks
            .enumerate()
            // TODO: ideally we don't clone here but we take ownership of the output of writer
            .map(|(fragment_index, chunk)| FragmentData {
                message_id: fragment_message_id,
                fragment_id: FragmentIndex(fragment_index as u64),
                num_fragments: FragmentIndex(num_fragments as u64),
                fragment_size: self.fragment_size,
                compression: (fragment_index == 0).then_some(compression),
                bytes: fragment_bytes.slice_ref(chunk),
            })
            .collect::<_>()
    }
}

fn fragment_compression(compression: CompressionConfig) -> Option<FragmentCompression> {
    match compression.algorithm {
        #[cfg(feature = "compression_lz4")]
        Some(crate::packet::compression::CompressionAlgorithm::Lz4) => {
            Some(FragmentCompression::Lz4)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use bytes::Bytes;

    use crate::packet::packet::FRAGMENT_SIZE;

    use super::*;

    #[test]
    fn test_build_fragments() {
        let message_id = MessageId(0);
        const NUM_BYTES: usize = (FRAGMENT_SIZE as f32 * 2.5) as usize;
        let bytes = Bytes::from(vec![0; NUM_BYTES]);

        let sender = FragmentSender::new();

        let fragments = sender.build_fragments(message_id, bytes.clone());
        let expected_num_fragments = 3;
        assert_eq!(fragments.len(), expected_num_fragments);
        assert_eq!(
            fragments.first().unwrap(),
            &FragmentData {
                message_id,
                fragment_id: FragmentIndex(0),
                num_fragments: FragmentIndex(expected_num_fragments as u64),
                fragment_size: FRAGMENT_SIZE,
                compression: Some(FragmentCompression::None),
                bytes: bytes.slice(0..FRAGMENT_SIZE),
            }
        );
        assert_eq!(
            fragments.get(1).unwrap(),
            &FragmentData {
                message_id,
                fragment_id: FragmentIndex(1),
                num_fragments: FragmentIndex(expected_num_fragments as u64),
                fragment_size: FRAGMENT_SIZE,
                compression: None,
                bytes: bytes.slice(FRAGMENT_SIZE..2 * FRAGMENT_SIZE),
            }
        );
        assert_eq!(
            fragments.get(2).unwrap(),
            &FragmentData {
                message_id,
                // tick: None,
                fragment_id: FragmentIndex(2),
                num_fragments: FragmentIndex(expected_num_fragments as u64),
                fragment_size: FRAGMENT_SIZE,
                compression: None,
                bytes: bytes.slice(2 * FRAGMENT_SIZE..),
            }
        );
    }
}
