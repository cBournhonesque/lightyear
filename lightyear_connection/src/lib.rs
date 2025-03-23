/*! # Lightyear Connection

Connection handling for the lightyear networking library.
This crate provides abstractions for managing long-term connections.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod client;
pub mod netcode;

pub mod server;

pub mod id;
mod local;
#[cfg_attr(docsrs, doc(cfg(all(feature = "steam", not(target_family = "wasm")))))]
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
pub mod steam;
