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
use crate::packet::packet::{HEADER_BYTES, Packet, PacketCompressionInfo};
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

/// Transport-wide compression workspace retained across compression attempts.
///
/// LZ4's hash table is shared across compression operations. [`Self::compress`] writes into the
/// owned output buffer, while [`Self::compress_into`] allows callers to retain their own buffer.
#[derive(Default)]
pub(crate) struct CompressionScratch {
    output: Vec<u8>,
    #[cfg(feature = "compression_lz4")]
    lz4_table: Option<lz4_flex::block::CompressTable>,
}

impl core::fmt::Debug for CompressionScratch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut state = formatter.debug_struct("CompressionScratch");
        state.field("output_capacity", &self.output.capacity());
        #[cfg(feature = "compression_lz4")]
        state.field("lz4_table_initialized", &self.lz4_table.is_some());
        state.finish()
    }
}

impl CompressionScratch {
    fn compress_into<'output>(
        &mut self,
        payload: &[u8],
        algorithm: CompressionAlgorithm,
        output: &'output mut Vec<u8>,
    ) -> Result<&'output [u8], PacketError> {
        match algorithm {
            #[cfg(feature = "compression_lz4")]
            CompressionAlgorithm::Lz4 => Self::compress_lz4(
                payload,
                output,
                self.lz4_table
                    .get_or_insert_with(lz4_flex::block::CompressTable::default),
            ),
        }
    }

    fn compress(
        &mut self,
        payload: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<&[u8], PacketError> {
        match algorithm {
            #[cfg(feature = "compression_lz4")]
            CompressionAlgorithm::Lz4 => Self::compress_lz4(
                payload,
                &mut self.output,
                self.lz4_table
                    .get_or_insert_with(lz4_flex::block::CompressTable::default),
            ),
        }
    }

    fn take_output(&mut self, len: usize) -> Vec<u8> {
        debug_assert!(len <= self.output.len());
        self.output.truncate(len);
        core::mem::take(&mut self.output)
    }

    #[cfg(feature = "compression_lz4")]
    fn compress_lz4<'output>(
        payload: &[u8],
        output: &'output mut Vec<u8>,
        table: &mut lz4_flex::block::CompressTable,
    ) -> Result<&'output [u8], PacketError> {
        const SIZE_PREFIX_BYTES: usize = core::mem::size_of::<u32>();

        let uncompressed_len =
            u32::try_from(payload.len()).map_err(|_| PacketError::CompressionFailed)?;
        let output_len =
            SIZE_PREFIX_BYTES + lz4_flex::block::get_maximum_output_size(payload.len());
        if output.len() < output_len {
            output.resize(output_len, 0);
        }
        output[..SIZE_PREFIX_BYTES].copy_from_slice(&uncompressed_len.to_le_bytes());

        let compressed_len = lz4_flex::block::compress_into_with_table(
            payload,
            &mut output[SIZE_PREFIX_BYTES..output_len],
            table,
        )
        .map_err(|_| PacketError::CompressionFailed)?;

        Ok(&output[..SIZE_PREFIX_BYTES + compressed_len])
    }
}

/// Compress a packet payload body into reusable output without modifying the packet.
///
/// This is also used while packing messages to preserve the behavior of admitting an
/// uncompressed packet larger than the MTU when its compressed representation still fits.
pub(crate) fn evaluate_packet_compression(
    packet_payload: &[u8],
    config: CompressionConfig,
    mtu: usize,
    scratch: &mut CompressionScratch,
    output: &mut Vec<u8>,
) -> Result<CompressionOutcome, PacketError> {
    let Some(algorithm) = config.algorithm else {
        return Ok(CompressionOutcome::Disabled);
    };

    if packet_payload.len() <= HEADER_BYTES {
        return Ok(CompressionOutcome::TooSmall { payload_len: 0 });
    }

    let packet_type = PacketType::try_from(packet_payload[PacketHeader::PACKET_TYPE_OFFSET])?;
    if packet_type.compressed_variant().is_none() {
        return Ok(CompressionOutcome::AlreadyCompressed);
    }

    let payload_len = packet_payload.len() - HEADER_BYTES;
    if payload_len < config.min_payload_size {
        return Ok(CompressionOutcome::TooSmall { payload_len });
    }
    if payload_len > config.max_decompressed_payload_size {
        return Ok(CompressionOutcome::TooLargeForDecompressionLimit {
            payload_len,
            limit: config.max_decompressed_payload_size,
        });
    }

    let compressed_payload =
        scratch.compress_into(&packet_payload[HEADER_BYTES..], algorithm, output)?;
    let original_len = packet_payload.len();
    let compressed_len = HEADER_BYTES + compressed_payload.len();
    if compressed_len >= original_len || compressed_len > mtu {
        return Ok(CompressionOutcome::NotSmaller {
            original_len,
            compressed_len,
        });
    }

    Ok(CompressionOutcome::Compressed {
        original_len,
        compressed_len,
    })
}

/// Compress the packet payload body in place when compression is beneficial.
///
/// The packet is only modified if the compressed final packet is strictly smaller than the original
/// packet and still fits in the packet MTU.
pub(crate) fn compress_packet(
    packet: &mut Packet,
    config: CompressionConfig,
    mtu: usize,
    scratch: &mut CompressionScratch,
    output: &mut Vec<u8>,
) -> Result<CompressionOutcome, PacketError> {
    let outcome = evaluate_packet_compression(&packet.payload, config, mtu, scratch, output)?;
    let CompressionOutcome::Compressed {
        original_len,
        compressed_len,
    } = outcome
    else {
        return Ok(outcome);
    };

    let packet_type = PacketType::try_from(packet.payload[PacketHeader::PACKET_TYPE_OFFSET])?;
    let compressed_packet_type = packet_type
        .compressed_variant()
        .expect("compression evaluation rejects already-compressed packet types");
    let compressed_payload_len = compressed_len - HEADER_BYTES;
    packet.payload[PacketHeader::PACKET_TYPE_OFFSET] = compressed_packet_type.into();
    packet.payload.truncate(HEADER_BYTES);
    packet
        .payload
        .extend_from_slice(&output[..compressed_payload_len]);
    packet.compression = Some(PacketCompressionInfo {
        original_len,
        compressed_len,
    });
    Ok(outcome)
}

pub(crate) fn compress_fragment(
    payload: &[u8],
    config: CompressionConfig,
    scratch: &mut CompressionScratch,
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

    let compressed_len = scratch.compress(payload, algorithm)?.len();

    if compressed_len >= payload_len {
        return Ok(PayloadCompressionCandidate::NotSmaller {
            original_len: payload_len,
            compressed_len,
        });
    }

    Ok(PayloadCompressionCandidate::Compressed {
        payload: scratch.take_output(compressed_len),
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
        let mut scratch = CompressionScratch::default();
        let mut output = Vec::new();

        let outcome = compress_packet(
            &mut packet,
            CompressionConfig::DISABLED,
            1200,
            &mut scratch,
            &mut output,
        )
        .unwrap();

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
        let original_capacity = packet.payload.capacity();
        let original_pointer = packet.payload.as_ptr();
        let mut scratch = CompressionScratch::default();
        let mut output = Vec::new();

        let outcome =
            compress_packet(&mut packet, config, 1200, &mut scratch, &mut output).unwrap();

        assert!(matches!(outcome, CompressionOutcome::Compressed { .. }));
        assert!(packet.payload.len() < original_len);
        assert_eq!(packet.payload.capacity(), original_capacity);
        assert_eq!(packet.payload.as_ptr(), original_pointer);
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
        let mut scratch = CompressionScratch::default();
        let mut output = Vec::new();

        let outcome =
            compress_packet(&mut packet, config, 1200, &mut scratch, &mut output).unwrap();

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
