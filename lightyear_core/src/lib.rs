//! # Lightyear Core
//!
//! This crate provides fundamental types and utilities shared across the Lightyear networking library.
//! It includes core concepts such as:
//! - Ticking and time management (`tick`, `time`, `timeline`).
//! - Network identifiers and abstractions (`network`, `id`).
//! - History buffers for state management (`history_buffer`).
//! - Core plugin structures (`plugin`).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

/// Defines the `Tick` type and related systems for managing discrete time steps.
pub mod tick;


/// Provides core network-related types and traits.
pub mod network;

/// Defines core plugin structures and related utilities.
pub mod plugin;
/// Utilities for time management, including interpolation and synchronization.
pub mod time;
/// Provides `HistoryBuffer` for storing and managing historical state.
pub mod history_buffer;
/// Defines `Timeline` for managing different views of time (local, network).
pub mod timeline;
/// Provides types for network identifiers, such as `PeerId` and `NetId`.
pub mod id;

/// Commonly used items from the `lightyear_core` crate.
pub mod prelude {
    pub use crate::id::PeerId;
    pub use crate::tick::Tick;
    pub use crate::timeline::{LocalTimeline, NetworkTimeline, NetworkTimelinePlugin, RollbackState, Timeline};
}