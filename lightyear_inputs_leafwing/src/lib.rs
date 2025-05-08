//! # Lightyear Inputs Leafwing
//!
//! This crate provides an integration between `lightyear` and `leafwing-input-manager`.
//!
//! It allows you to use `leafwing-input-manager`'s `ActionState` as the input type for `lightyear`.
//! The inputs are sent from the client to the server and are predicted on the client.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

pub(crate) mod action_diff;

mod action_state;

mod input_message;

mod plugin;

pub mod prelude {
    pub use crate::plugin::InputPlugin;
}