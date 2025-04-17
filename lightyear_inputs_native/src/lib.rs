/*! # Lightyear Native Inputs
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub(crate) mod action_state;
#[cfg(feature = "client")]
mod client;

pub(crate) mod input_buffer;

pub(crate) mod input_message;

pub mod plugin;

#[cfg(feature = "server")]
mod server;