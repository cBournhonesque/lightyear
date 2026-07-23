//! Fragment construction and acknowledgement tracking for channel sends.

mod acknowledgements {
    use crate::packet::message::{FragmentIndex, MessageId};
    use alloc::{vec, vec::Vec};
    use bevy_platform::collections::HashMap;
    use core::time::Duration;
    use tracing::trace;

    /// Tracks acknowledgements for the fragments of one or more messages.
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

        pub fn add_new_fragment_to_wait_for(
            &mut self,
            message_id: MessageId,
            num_fragments: usize,
        ) {
            self.fragment_messages
                .entry(message_id)
                .or_insert_with(|| FragmentAckTracker::new(num_fragments));
        }

        pub fn discard_message(&mut self, message_id: MessageId) {
            self.fragment_messages.remove(&message_id);
        }

        /// Discard all messages for which the latest ack was received before the cleanup time
        /// (i.e. we probably lost some fragments and we will never get all the acks for this fragmented message)
        ///
        /// If we don't keep track of the last received time, we will never clean up the messages.
        pub fn cleanup(&mut self, cleanup_time: Duration) {
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
            current_time: Option<Duration>,
        ) -> bool {
            let Some(fragment_ack_tracker) = self.fragment_messages.get_mut(&message_id) else {
                trace!(?message_id, "ignoring fragment ACK without pending state");
                return false;
            };

            // completed the fragmented message!
            if fragment_ack_tracker.receive_ack(fragment_index.0 as usize, current_time) {
                self.fragment_messages.remove(&message_id);
                return true;
            }

            false
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    /// Data structure to keep track of when an entire fragment message is acked
    struct FragmentAckTracker {
        num_fragments: usize,
        num_received_fragments: usize,
        received: Vec<bool>,
        last_received: Option<Duration>,
    }

    impl FragmentAckTracker {
        fn new(num_fragments: usize) -> Self {
            Self {
                num_fragments,
                num_received_fragments: 0,
                received: vec![false; num_fragments],
                last_received: None,
            }
        }

        /// Receive a fragment index ack, and return true if the entire fragment was acked.
        fn receive_ack(&mut self, fragment_index: usize, received_time: Option<Duration>) -> bool {
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

            assert!(!receiver.receive_fragment_ack(MessageId(0), FragmentIndex(0), None));
            // receiving the same fragment twice should do nothing
            assert!(!receiver.receive_fragment_ack(MessageId(0), FragmentIndex(0), None));
            // we receive the entire fragment: should return true
            assert!(receiver.receive_fragment_ack(MessageId(0), FragmentIndex(1), None));

            assert!(receiver.fragment_messages.is_empty());
        }

        #[test]
        fn test_cleanup() {
            let mut receiver = FragmentAckReceiver::new();

            receiver.add_new_fragment_to_wait_for(MessageId(0), 2);

            assert!(!receiver.receive_fragment_ack(
                MessageId(0),
                FragmentIndex(0),
                Some(Duration::from_millis(150))
            ));
            receiver.cleanup(Duration::from_millis(170));
            assert!(receiver.fragment_messages.is_empty());
        }
    }
}

mod fragmentation {
    use crate::packet::compression::{
        CompressionConfig, CompressionScratch, PayloadCompressionCandidate, compress_fragment,
    };
    use crate::packet::message::{FragmentCompression, FragmentData, FragmentIndex, MessageId};
    use crate::packet::packet::FRAGMENT_SIZE;
    use alloc::vec::Vec;
    use bytes::Bytes;
    use tracing::trace;

    /// Splits oversized messages into transport fragments.
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
            &mut self,
            fragment_message_id: MessageId,
            message: Bytes,
            compression: CompressionConfig,
            compression_scratch: &mut CompressionScratch,
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
            }) = compress_fragment(message.as_ref(), compression, compression_scratch)
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
                    compression: None,
                    bytes: bytes.slice(2 * FRAGMENT_SIZE..),
                }
            );
        }
    }
}

pub(crate) use acknowledgements::FragmentAckReceiver;
pub(crate) use fragmentation::FragmentSender;
