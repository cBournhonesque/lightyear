//! Peer-to-peer session tests.
//!
//! A P2P session has no authority: every app is a peer. These tests drive the real
//! [`P2PSessionPlugin`](lightyear_p2p::P2PSessionPlugin) lifecycle in one process. Multi-peer tests
//! that need real Links live with the transport that provides them.

mod solo;
