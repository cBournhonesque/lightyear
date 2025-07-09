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

/// Messages exchanged betwen client and server
pub mod messages;

/// Commonly used items from the `lightyear_core` crate.
pub mod prelude {}
