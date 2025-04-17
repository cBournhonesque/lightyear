/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod action_diff;

mod action_state;

mod input_buffer;

mod input_message;
#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;
mod plugin;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::{Component, SystemSet};
