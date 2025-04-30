#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::*;

#[cfg(feature = "server")]
pub mod server;

pub mod client;

pub use client::ClientWebTransportPlugin;

#[cfg(feature = "server")]
pub use server::ServerWebTransportPlugin;

// Re-export client_wasm for WASM targets
#[cfg(target_family = "wasm")]
pub mod client_wasm;
