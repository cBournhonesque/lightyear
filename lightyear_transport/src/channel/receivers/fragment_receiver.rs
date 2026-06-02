use alloc::{vec, vec::Vec};
use bevy_platform::collections::HashMap;

use crate::channel::receivers::error::{ChannelReceiveError, Result};
#[cfg(feature = "compression_lz4")]
use crate::packet::compression::CompressionAlgorithm;
use crate::packet::compression::{CompressionConfig, decompress_payload};
use crate::packet::error::PacketError;
use crate::packet::message::{FragmentCompression, FragmentData, MessageId};
use crate::packet::packet::FRAGMENT_SIZE;
use bytes::Bytes;
use core::time::Duration;
use lightyear_core::tick::Tick;
use tracing::trace;

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
    pub fn cleanup(&mut self, cleanup_time: Duration) {
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
        current_time: Option<Duration>,
        compression: CompressionConfig,
    ) -> Result<Option<(Tick, Bytes)>> {
        let num_fragments = fragment.num_fragments.0;
        let fragment_index = fragment.fragment_id.0;
        if num_fragments == 0 {
            return Err(ChannelReceiveError::InvalidFragmentCount { num_fragments });
        }
        if fragment_index >= num_fragments {
            return Err(ChannelReceiveError::InvalidFragmentIndex {
                fragment_index,
                num_fragments,
            });
        }

        let message_id = fragment.message_id;
        let fragment_compression = fragment.compression;
        let fragment_message = self.fragment_messages.entry(message_id).or_insert_with(|| {
            FragmentConstructor::new(
                remote_sent_tick,
                num_fragments as usize,
                fragment_compression,
            )
        });
        if fragment_message.num_fragments != num_fragments as usize {
            return Err(ChannelReceiveError::FragmentCountMismatch {
                expected: fragment_message.num_fragments,
                actual: num_fragments as usize,
            });
        }
        if fragment_message.compression != fragment_compression {
            return Err(ChannelReceiveError::FragmentCompressionMismatch {
                expected: fragment_message.compression.as_str(),
                actual: fragment_compression.as_str(),
            });
        }

        // completed the fragmented message!
        if let Some(payload) = fragment_message.receive_fragment(
            fragment_index as usize,
            fragment.bytes.as_ref(),
            current_time,
        )? {
            self.fragment_messages.remove(&message_id);
            let (tick, payload) = payload;
            return decompress_fragment_payload(fragment_compression, payload, compression)
                .map(|payload| Some((tick, payload)));
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

    tick: Tick,
    compression: FragmentCompression,
    last_received: Option<Duration>,
}

impl FragmentConstructor {
    pub fn new(tick: Tick, num_fragments: usize, compression: FragmentCompression) -> Self {
        Self {
            num_fragments,
            num_received_fragments: 0,
            received: vec![false; num_fragments],
            bytes: vec![0; num_fragments * FRAGMENT_SIZE],
            tick,
            compression,
            last_received: None,
        }
    }

    pub fn receive_fragment(
        &mut self,
        fragment_index: usize,
        bytes: &[u8],
        received_time: Option<Duration>,
    ) -> Result<Option<(Tick, Bytes)>> {
        self.last_received = received_time;

        let is_last_fragment = fragment_index == self.num_fragments - 1;

        if !is_last_fragment && bytes.len() != FRAGMENT_SIZE {
            return Err(ChannelReceiveError::InvalidNonFinalFragmentSize {
                actual: bytes.len(),
                expected: FRAGMENT_SIZE,
            });
        }
        if is_last_fragment && bytes.len() > FRAGMENT_SIZE {
            return Err(ChannelReceiveError::InvalidFinalFragmentSize {
                actual: bytes.len(),
                max: FRAGMENT_SIZE,
            });
        }

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
            return Ok(Some((self.tick, payload.into())));
        }

        Ok(None)
    }
}

fn decompress_fragment_payload(
    compression: FragmentCompression,
    payload: Bytes,
    config: CompressionConfig,
) -> Result<Bytes> {
    match compression {
        FragmentCompression::None => Ok(payload),
        FragmentCompression::Lz4 => {
            let config = config_for_fragment_compression(compression, config)?;
            decompress_payload(payload.as_ref(), config)
                .map(Bytes::from)
                .map_err(map_fragment_decompression_error)
        }
    }
}

fn config_for_fragment_compression(
    compression: FragmentCompression,
    config: CompressionConfig,
) -> Result<CompressionConfig> {
    match compression {
        FragmentCompression::None => Ok(config),
        FragmentCompression::Lz4 => {
            #[cfg(feature = "compression_lz4")]
            {
                if config.algorithm == Some(CompressionAlgorithm::Lz4) {
                    Ok(config)
                } else {
                    Err(ChannelReceiveError::UnsupportedFragmentCompression {
                        compression: compression.as_str(),
                    })
                }
            }
            #[cfg(not(feature = "compression_lz4"))]
            {
                Err(ChannelReceiveError::UnsupportedFragmentCompression {
                    compression: compression.as_str(),
                })
            }
        }
    }
}

fn map_fragment_decompression_error(error: PacketError) -> ChannelReceiveError {
    match error {
        PacketError::UnsupportedCompression => {
            ChannelReceiveError::UnsupportedFragmentCompression {
                compression: FragmentCompression::Lz4.as_str(),
            }
        }
        PacketError::DecompressionFailed => ChannelReceiveError::FragmentDecompressionFailed,
        PacketError::DecompressedPayloadTooLarge { actual, limit } => {
            ChannelReceiveError::FragmentDecompressedPayloadTooLarge { actual, limit }
        }
        _ => ChannelReceiveError::FragmentDecompressionFailed,
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
        let fragments = FragmentSender::new().build_fragments(MessageId(0), message_bytes.clone());

        assert_eq!(
            receiver.receive_fragment(
                fragments[0].clone(),
                Tick(0),
                None,
                CompressionConfig::DISABLED
            )?,
            None
        );
        assert_eq!(
            receiver.receive_fragment(
                fragments[1].clone(),
                Tick(1),
                None,
                CompressionConfig::DISABLED
            )?,
            Some((Tick(0), message_bytes.clone()))
        );
        Ok(())
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn compressed_fragments_are_decompressed_after_reassembly() -> Result<()> {
        let compression = CompressionConfig {
            min_payload_size: 0,
            max_decompressed_payload_size: FRAGMENT_SIZE * 4,
            ..CompressionConfig::LZ4
        };
        let mut receiver = FragmentReceiver::new();
        let message_bytes = Bytes::from(vec![7u8; FRAGMENT_SIZE * 3]);
        let fragments = FragmentSender::new().build_fragments_for_message(
            MessageId(1),
            message_bytes.clone(),
            compression,
        );

        assert!(fragments.len() < 3);
        assert!(
            fragments
                .iter()
                .all(|fragment| fragment.compression == FragmentCompression::Lz4)
        );

        let mut result = None;
        for (index, fragment) in fragments.into_iter().enumerate() {
            result = receiver.receive_fragment(fragment, Tick(index as u32), None, compression)?;
        }

        assert_eq!(result, Some((Tick(0), message_bytes)));
        Ok(())
    }
}
