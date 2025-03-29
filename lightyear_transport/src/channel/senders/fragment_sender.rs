use crate::packet::message::{FragmentData, FragmentIndex, MessageId};
use crate::packet::packet::FRAGMENT_SIZE;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bytes::Bytes;
use lightyear_core::tick::Tick;

/// `FragmentReceiver` is used to reconstruct fragmented messages
#[derive(Debug)]
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
        &self,
        fragment_message_id: MessageId,
        tick: Option<Tick>,
        fragment_bytes: Bytes,
    ) -> Vec<FragmentData> {
        if fragment_bytes.len() <= FRAGMENT_SIZE {
            unreachable!(
                "Message size must be at least {} to need to be fragmented",
                FRAGMENT_SIZE
            );
        }
        let chunks = fragment_bytes.chunks(self.fragment_size);
        let num_fragments = chunks.len();
        chunks
            .enumerate()
            // TODO: ideally we don't clone here but we take ownership of the output of writer
            .map(|(fragment_index, chunk)| FragmentData {
                message_id: fragment_message_id,
                // tick,
                fragment_id: FragmentIndex(fragment_index as u64),
                num_fragments: FragmentIndex(num_fragments as u64),
                bytes: fragment_bytes.slice_ref(chunk),
            })
            .collect::<_>()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
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

        let fragments = sender
            .build_fragments(message_id, None, bytes.clone());
        let expected_num_fragments = 3;
        assert_eq!(fragments.len(), expected_num_fragments);
        assert_eq!(
            fragments.first().unwrap(),
            &FragmentData {
                message_id,
                fragment_id: FragmentIndex(0),
                num_fragments: FragmentIndex(expected_num_fragments as u64),
                bytes: bytes.slice(0..FRAGMENT_SIZE),
            }
        );
        assert_eq!(
            fragments.get(1).unwrap(),
            &FragmentData {
                message_id,
                fragment_id: FragmentIndex(1),
                num_fragments: FragmentIndex(expected_num_fragments as u64),
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
                bytes: bytes.slice(2 * FRAGMENT_SIZE..),
            }
        );
    }
}
