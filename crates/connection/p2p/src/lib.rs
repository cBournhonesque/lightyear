//! Deterministic peer-to-peer session lifecycle for Lightyear.
//!
//! Applications declare ordinary [`P2P`](lightyear_connection::p2p::P2P) Links, then trigger
//! [`P2PStart`] when the desired cohort is present. The session waits for those Links and the
//! synchronized input timeline, then chooses one shared future start tick.
//!
//! The session does not own peer discovery, Link connection, or network topology. Stopping it can
//! either preserve every Link for a lobby/rematch or unlink every currently declared P2P Link.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod session;

pub use session::*;

/// Commonly used P2P session types.
pub mod prelude {
    pub use crate::{
        P2PSession, P2PSessionPlugin, P2PSessionState, P2PStart, P2PStarted, P2PStop, P2PStopped,
    };
}
