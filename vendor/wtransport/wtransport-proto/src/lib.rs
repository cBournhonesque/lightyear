//! WebTransport protocol implementation.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs, clippy::doc_markdown)]

/// I/O and buffer operations.
pub mod bytes;

/// HTTP3 datagrams.
pub mod datagram;

/// Errors definitions.
pub mod error;

/// HTTP3 frame.
pub mod frame;

/// HTTP3 HEADERS frame payload.
pub mod headers;

/// Types for identifiers.
pub mod ids;

/// WebTransport session utilities.
pub mod session;

/// HTTP3 SETTINGS frame payload.
pub mod settings;

/// HTTP3 stream types.
pub mod stream;

/// HTTP3 stream header.
pub mod stream_header;

/// QUIC variable-length integer.
pub mod varint;

/// Application Layer Protocol Negotiation for WebTransport connections.
pub const WEBTRANSPORT_ALPN: &[u8; 2] = b"h3";
