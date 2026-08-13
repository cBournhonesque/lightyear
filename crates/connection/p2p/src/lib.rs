//! Deterministic peer-to-peer session lifecycle for Lightyear.
//!
//! Applications declare [`P2P::Inactive`](lightyear_connection::p2p::P2P::Inactive) Links, then
//! trigger [`P2PStart`] when the desired cohort is present. The current inactive Links become
//! candidates; the session waits for them and the synchronized input timeline, chooses one shared
//! future start tick, then marks them joined.
//!
//! [`P2PSessionPlugin`] owns the barrier bookkeeping; applications do not need to configure a
//! [`P2PSession`]. The session does not own peer discovery or Link connection. Stopping it can
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
