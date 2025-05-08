//! # Lightyear WebTransport
//!
//! This crate provides a WebTransport transport layer for Lightyear.
//! WebTransport is a modern, low-latency, bidirectional protocol built on top of HTTP/3,
//! suitable for web-based real-time applications.
//!
//! This crate offers:
//! - `ClientWebTransportPlugin`: For integrating WebTransport as a client in Bevy applications.
//! - `ServerWebTransportPlugin` (when "server" feature is enabled): For integrating WebTransport
//!   as a server.
//! - WASM-specific client implementations via the `client_wasm` module.
//!
//! It allows Lightyear to communicate over WebTransport, leveraging its benefits for
//! browser-based games or applications requiring web compatibility.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::*;

/// Provides server-side WebTransport functionalities.
/// This module is only available when the "server" feature is enabled.
#[cfg(feature = "server")]
pub mod server;

/// Provides client-side WebTransport functionalities.
pub mod client;

pub use client::ClientWebTransportPlugin;

#[cfg(feature = "server")]
pub use server::ServerWebTransportPlugin;

/// Provides WASM-specific client-side WebTransport functionalities.
/// This module is only available when targeting WASM.
// Re-export client_wasm for WASM targets
#[cfg(target_family = "wasm")]
pub mod client_wasm;
