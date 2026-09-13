//! Peer-to-peer tests.
//!
//! Unlike the client/server stepper, a P2P session has no authority: every app is a peer that owns
//! one Link per remote peer. These tests drive a real deterministic P2P session with lobby-based
//! peer discovery, over the UDP IO layer.

mod lobby;
#[cfg(feature = "p2p_websocket")]
mod websocket;
#[cfg(feature = "p2p_webtransport")]
mod webtransport;
