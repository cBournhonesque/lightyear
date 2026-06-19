//! # Lightyear Deterministic Replication
//!
//! Utilities for lockstep-style deterministic simulation on top of Lightyear:
//! - [`ChecksumSendPlugin`] / [`ChecksumReceivePlugin`] compute and verify
//!   XOR checksums of prediction history across client and server.
//! - [`LateJoinCatchUpPlugin`] lets a client that connects mid-game request
//!   a one-time snapshot of a remote entity's state so it can fast-forward
//!   to the current tick via a forced rollback.
//! - [`DeterministicReplicationPlugin`] wires up the shared archetype
//!   index used by both features.
//!
//! [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
//! [`ChecksumReceivePlugin`]: crate::prelude::ChecksumReceivePlugin
//! [`LateJoinCatchUpPlugin`]: crate::prelude::LateJoinCatchUpPlugin
//! [`DeterministicReplicationPlugin`]: crate::prelude::DeterministicReplicationPlugin

#![no_std]

extern crate alloc;
extern crate core;
#[cfg(test)]
extern crate std;

use bevy_ecs::component::Component;

mod archetypes;
mod checksum;
#[cfg(feature = "replication")]
/// Late-join catch-up: client-driven bundled snapshot replication so that
/// mid-game joiners can catch up to already-simulated entities.
pub mod late_join;

/// Configuration mode for catch-up (state-based vs input-only).
pub mod mode;
mod plugin;

/// Commonly used items from the `lightyear_deterministic_replication` crate.
pub mod prelude {
    pub use lightyear_prediction::rollback::CatchUpGated;
    pub use crate::checksum::ChecksumMessage;
    #[cfg(feature = "client")]
    pub use crate::checksum::ChecksumSendPlugin;
    #[cfg(feature = "server")]
    pub use crate::checksum::{ChecksumHistory, ChecksumReceivePlugin};
    #[cfg(feature = "replication")]
    pub use crate::late_join::{
        AppCatchUpExt, CatchUpRegistry, CatchUpRequest,
        CatchUpSnapshotReady, CatchUpSystems, HasCaughtUp, LateJoinCatchUpPlugin,
    };
    #[cfg(all(feature = "client", feature = "replication"))]
    pub use crate::late_join::{CatchUpClientTimeout, CatchUpManager};
    pub use crate::mode::CatchUpMode;
    pub use crate::plugin::DeterministicReplicationPlugin;
}

/// Marker component that indicates that this entity is deterministic. It is not updated via state, but only via inputs.
#[derive(Component, Default)]
pub struct Deterministic;
