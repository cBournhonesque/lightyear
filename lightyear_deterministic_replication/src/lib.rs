//! # Lightyear Core
//!
//! This crate provides fundamental types and utilities shared across the Lightyear networking library.
//! It includes core concepts such as:
//! - Ticking and time management (`tick`, `time`, `timeline`).
//! - Network identifiers and abstractions (`network`, `id`).
//! - History buffers for state management (`history_buffer`).
//! - Core plugin structures (`plugin`).

#![no_std]

extern crate alloc;
extern crate core;
#[cfg(test)]
extern crate std;

use bevy_ecs::component::Component;

mod archetypes;
mod checksum;
/// Late-join catch-up: client-driven per-component snapshot replication
/// so that mid-game joiners can catch up to already-simulated entities.
pub mod late_join;
/// Messages exchanged between client and server
pub mod messages;
mod plugin;

/// Commonly used items from the `lightyear_core` crate.
pub mod prelude {
    pub use crate::checksum::{
        ChecksumHistory, ChecksumMessage, ChecksumReceivePlugin, ChecksumSendPlugin,
    };
    pub use crate::late_join::{
        AppCatchUpExt, CatchUpBit, CatchUpForEntity, CatchUpGated, CatchUpReady, CatchUpRegistry,
        LateJoinCatchUpPlugin, PendingCatchUp, apply_catch_up_for_entity,
        request_forced_rollback_from_confirm_history,
    };
    pub use crate::plugin::DeterministicReplicationPlugin;
}

/// Marker component that indicates that this entity is deterministic. It is not updated via state, but only via inputs.
#[derive(Component, Default)]
pub struct Deterministic;
