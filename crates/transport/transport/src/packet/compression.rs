//! Packet payload compression helpers.
//!
//! Compression is applied after packet headers have been written. The header stays
//! uncompressed so receivers can parse packet ids, acks, ticks, and the packet type
//! before deciding whether the payload body needs decompression.

#[cfg(feature = "compression_lz4")]
use alloc::vec;
use alloc::vec::Vec;

use crate::packet::error::PacketError;
use crate::packet::header::PacketHeader;
use crate::packet::packet::{HEADER_BYTES, MAX_PACKET_SIZE, Packet, PacketCompressionInfo};
use crate::packet::packet_type::PacketType;

const DEFAULT_MIN_PAYLOAD_SIZE: usize = 128;
const DEFAULT_MAX_DECOMPRESSED_PAYLOAD_SIZE: usize = 64 * 1024;

/// Compression algorithm used for transport packet payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    #[cfg(feature = "compression_lz4")]
    Lz4,
}

/// Configuration for post-pack packet payload compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressionConfig {
    /// `None` disables compression.
    pub algorithm: Option<CompressionAlgorithm>,
    /// Do not attempt compression for payload bodies smaller than this value.
    pub min_payload_size: usize,
    /// Maximum decompressed packet payload body accepted on receive.
    pub max_decompressed_payload_size: usize,
}

impl CompressionConfig {
    pub const DISABLED: Self = Self {
        algorithm: None,
        min_payload_size: DEFAULT_MIN_PAYLOAD_SIZE,
        max_decompressed_payload_size: DEFAULT_MAX_DECOMPRESSED_PAYLOAD_SIZE,
    };

    #[cfg(feature = "compression_lz4")]
    pub const LZ4: Self = Self {
        algorithm: Some(CompressionAlgorithm::Lz4),
        min_payload_size: DEFAULT_MIN_PAYLOAD_SIZE,
        max_decompressed_payload_size: DEFAULT_MAX_DECOMPRESSED_PAYLOAD_SIZE,
    };

    pub const fn is_enabled(&self) -> bool {
        self.algorithm.is_some()
    }
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self::DISABLED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompressionOutcome {
    Disabled,
    AlreadyCompressed,
    TooSmall {
        payload_len: usize,
    },
    TooLargeForDecompressionLimit {
        payload_len: usize,
        limit: usize,
    },
    NotSmaller {
        original_len: usize,
        compressed_len: usize,
    },
    Compressed {
        original_len: usize,
        compressed_len: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CompressionCandidate {
    Disabled,
    AlreadyCompressed,
    TooSmall {
        payload_len: usize,
    },
    TooLargeForDecompressionLimit {
        payload_len: usize,
        limit: usize,
    },
    NotSmaller {
        original_len: usize,
        compressed_len: usize,
    },
    Compressed {
        payload: Vec<u8>,
        original_len: usize,
        compressed_len: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PayloadCompressionCandidate {
    Disabled,
    TooSmall {
        payload_len: usize,
    },
    TooLargeForDecompressionLimit {
        payload_len: usize,
        limit: usize,
    },
    NotSmaller {
        original_len: usize,
        compressed_len: usize,
    },
    Compressed {
        payload: Vec<u8>,
        original_len: usize,
        compressed_len: usize,
    },
}

/// Try to compress the packet payload body in place.
///
/// The packet is only modified if the compressed final packet is strictly smaller than the original
/// packet and still fits in the packet MTU.
pub(crate) fn try_compress_packet(
    packet: &mut Packet,
    config: CompressionConfig,
) -> Result<CompressionOutcome, PacketError> {
    match try_build_compressed_packet_payload(&packet.payload, config)? {
        CompressionCandidate::Disabled => Ok(CompressionOutcome::Disabled),
        CompressionCandidate::AlreadyCompressed => Ok(CompressionOutcome::AlreadyCompressed),
        CompressionCandidate::TooSmall { payload_len } => {
            Ok(CompressionOutcome::TooSmall { payload_len })
        }
        CompressionCandidate::TooLargeForDecompressionLimit { payload_len, limit } => {
            Ok(CompressionOutcome::TooLargeForDecompressionLimit { payload_len, limit })
        }
        CompressionCandidate::NotSmaller {
            original_len,
            compressed_len,
        } => Ok(CompressionOutcome::NotSmaller {
            original_len,
            compressed_len,
        }),
        CompressionCandidate::Compressed {
            payload,
            original_len,
            compressed_len,
        } => {
            packet.payload = payload;
            packet.compression = Some(PacketCompressionInfo {
                original_len,
                compressed_len,
            });
            Ok(CompressionOutcome::Compressed {
                original_len,
                compressed_len,
            })
        }
    }
}

pub(crate) fn try_build_compressed_packet_payload(
    packet_payload: &[u8],
    config: CompressionConfig,
) -> Result<CompressionCandidate, PacketError> {
    if config.algorithm.is_none() {
        return Ok(CompressionCandidate::Disabled);
    }

    if packet_payload.len() <= HEADER_BYTES {
        return Ok(CompressionCandidate::TooSmall { payload_len: 0 });
    }

    let packet_type = PacketType::try_from(packet_payload[PacketHeader::PACKET_TYPE_OFFSET])?;
    let Some(compressed_packet_type) = packet_type.compressed_variant() else {
        return Ok(CompressionCandidate::AlreadyCompressed);
    };

    match try_build_compressed_payload(&packet_payload[HEADER_BYTES..], config)? {
        PayloadCompressionCandidate::Disabled => Ok(CompressionCandidate::Disabled),
        PayloadCompressionCandidate::TooSmall { payload_len } => {
            Ok(CompressionCandidate::TooSmall { payload_len })
        }
        PayloadCompressionCandidate::TooLargeForDecompressionLimit { payload_len, limit } => {
            Ok(CompressionCandidate::TooLargeForDecompressionLimit { payload_len, limit })
        }
        PayloadCompressionCandidate::NotSmaller {
            original_len,
            compressed_len,
        } => Ok(CompressionCandidate::NotSmaller {
            original_len: HEADER_BYTES + original_len,
            compressed_len: HEADER_BYTES + compressed_len,
        }),
        PayloadCompressionCandidate::Compressed {
            payload: compressed_payload,
            original_len,
            compressed_len,
        } => {
            let compressed_packet_len = HEADER_BYTES + compressed_len;
            let original_packet_len = HEADER_BYTES + original_len;
            if compressed_packet_len > MAX_PACKET_SIZE {
                return Ok(CompressionCandidate::NotSmaller {
                    original_len: original_packet_len,
                    compressed_len: compressed_packet_len,
                });
            }

            let mut payload = Vec::with_capacity(compressed_packet_len);
            payload.extend_from_slice(&packet_payload[..HEADER_BYTES]);
            payload[PacketHeader::PACKET_TYPE_OFFSET] = compressed_packet_type.into();
            payload.extend_from_slice(&compressed_payload);

            Ok(CompressionCandidate::Compressed {
                payload,
                original_len: original_packet_len,
                compressed_len: compressed_packet_len,
            })
        }
    }
}

pub(crate) fn try_build_compressed_payload(
    payload: &[u8],
    config: CompressionConfig,
) -> Result<PayloadCompressionCandidate, PacketError> {
    let Some(algorithm) = config.algorithm else {
        return Ok(PayloadCompressionCandidate::Disabled);
    };

    let payload_len = payload.len();
    if payload_len < config.min_payload_size {
        return Ok(PayloadCompressionCandidate::TooSmall { payload_len });
    }
    if payload_len > config.max_decompressed_payload_size {
        return Ok(PayloadCompressionCandidate::TooLargeForDecompressionLimit {
            payload_len,
            limit: config.max_decompressed_payload_size,
        });
    }

    let compressed_payload = compress_payload(payload, algorithm);
    let compressed_len = compressed_payload.len();

    if compressed_len >= payload_len {
        return Ok(PayloadCompressionCandidate::NotSmaller {
            original_len: payload_len,
            compressed_len,
        });
    }

    Ok(PayloadCompressionCandidate::Compressed {
        payload: compressed_payload,
        original_len: payload_len,
        compressed_len,
    })
}

pub(crate) fn decompress_payload(
    compressed_payload: &[u8],
    config: CompressionConfig,
) -> Result<Vec<u8>, PacketError> {
    let Some(algorithm) = config.algorithm else {
        return Err(PacketError::UnsupportedCompression);
    };

    match algorithm {
        #[cfg(feature = "compression_lz4")]
        CompressionAlgorithm::Lz4 => decompress_lz4(compressed_payload, config),
    }
}

fn compress_payload(payload: &[u8], algorithm: CompressionAlgorithm) -> Vec<u8> {
    match algorithm {
        #[cfg(feature = "compression_lz4")]
        CompressionAlgorithm::Lz4 => lz4_flex::block::compress_prepend_size(payload),
    }
}

#[cfg(feature = "compression_lz4")]
fn decompress_lz4(
    compressed_payload: &[u8],
    config: CompressionConfig,
) -> Result<Vec<u8>, PacketError> {
    let (uncompressed_size, compressed_payload) =
        lz4_flex::block::uncompressed_size(compressed_payload)
            .map_err(|_| PacketError::DecompressionFailed)?;

    if uncompressed_size > config.max_decompressed_payload_size {
        return Err(PacketError::DecompressedPayloadTooLarge {
            actual: uncompressed_size,
            limit: config.max_decompressed_payload_size,
        });
    }

    let mut decompressed = vec![0; uncompressed_size];
    let decoded_size = lz4_flex::block::decompress_into(compressed_payload, &mut decompressed)
        .map_err(|_| PacketError::DecompressionFailed)?;
    if decoded_size != uncompressed_size {
        return Err(PacketError::DecompressionFailed);
    }

    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::header::PacketHeaderManager;
    use crate::packet::packet::{MessageMetadata, PacketId};
    use lightyear_core::tick::Tick;
    use lightyear_serde::ToBytes;

    fn packet_with_body(packet_type: PacketType, body: &[u8]) -> Packet {
        let header = PacketHeaderManager::new(1.5).preview_send_packet_header(packet_type, Tick(0));
        let mut payload = Vec::new();
        header.to_bytes(&mut payload).unwrap();
        payload.extend_from_slice(body);

        Packet {
            payload,
            messages: Vec::<MessageMetadata>::new(),
            packet_id: PacketId(0),
            compression: None,
        }
    }

    #[test]
    fn disabled_compression_leaves_packet_unchanged() {
        let mut packet = packet_with_body(PacketType::Data, &[1, 2, 3, 4]);
        let original = packet.payload.clone();

        let outcome = try_compress_packet(&mut packet, CompressionConfig::DISABLED).unwrap();

        assert_eq!(outcome, CompressionOutcome::Disabled);
        assert_eq!(packet.payload, original);
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn lz4_compression_changes_packet_type_and_preserves_payload_body() {
        let original_body = vec![7u8; 512];
        let mut packet = packet_with_body(PacketType::Data, &original_body);
        let original_len = packet.payload.len();
        let config = CompressionConfig {
            min_payload_size: 0,
            ..CompressionConfig::LZ4
        };

        let outcome = try_compress_packet(&mut packet, config).unwrap();

        assert!(matches!(outcome, CompressionOutcome::Compressed { .. }));
        assert!(packet.payload.len() < original_len);
        assert_eq!(
            PacketType::try_from(packet.payload[PacketHeader::PACKET_TYPE_OFFSET]).unwrap(),
            PacketType::DataCompressed
        );
        let decompressed = decompress_payload(&packet.payload[HEADER_BYTES..], config).unwrap();
        assert_eq!(decompressed, original_body);
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn lz4_compression_falls_back_when_not_smaller() {
        let original_body = *b"small payload that should expand";
        let mut packet = packet_with_body(PacketType::Data, &original_body);
        let original = packet.payload.clone();
        let config = CompressionConfig {
            min_payload_size: 0,
            ..CompressionConfig::LZ4
        };

        let outcome = try_compress_packet(&mut packet, config).unwrap();

        assert!(matches!(outcome, CompressionOutcome::NotSmaller { .. }));
        assert_eq!(packet.payload, original);
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn lz4_decompression_respects_configured_size_cap() {
        let compressed = lz4_flex::block::compress_prepend_size(&[3u8; 32]);
        let config = CompressionConfig {
            max_decompressed_payload_size: 8,
            ..CompressionConfig::LZ4
        };

        let err = decompress_payload(&compressed, config).unwrap_err();

        assert!(matches!(
            err,
            PacketError::DecompressedPayloadTooLarge {
                actual: 32,
                limit: 8
            }
        ));
    }

    #[cfg(feature = "compression_lz4")]
    #[test]
    fn lz4_decompression_rejects_malformed_payload() {
        let err = decompress_payload(&[1, 2, 3], CompressionConfig::LZ4).unwrap_err();

        assert!(matches!(err, PacketError::DecompressionFailed));
    }
}
