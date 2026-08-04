//! Fixed-roster peer-to-peer session agreement for Lightyear.
//!
//! This crate sits above [`lightyear_connection`] and Lightyear's typed-message transport. It
//! validates that every direct [`P2P`](lightyear_connection::p2p::P2P) Link belongs to one agreed
//! roster, exchanges an application configuration fingerprint, and chooses a shared future start
//! tick after the existing input-timeline synchronization has completed.
//!
//! Timeline estimation and pacing remain in [`lightyear_sync`]. This crate only consumes
//! [`SyncedInputTimeline`](lightyear_sync::prelude::SyncedInputTimeline) as a readiness boundary;
//! it never calculates or applies a synchronization objective.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod session;

pub use session::*;

/// Commonly used P2P session types.
pub mod prelude {
    pub use crate::{
        P2PAgreement, P2PAgreementMismatch, P2PConfigFingerprint, P2PIdentityPolicy,
        P2PPeerLossPolicy, P2PSession, P2PSessionAborted, P2PSessionConfig, P2PSessionConfigError,
        P2PSessionError, P2PSessionId, P2PSessionPaused, P2PSessionPlugin,
        P2PSessionStartScheduled, P2PSessionStarted, P2PSessionState, p2p_session_running,
    };
}
