/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

#[cfg(test)]
mod client_server;
pub mod protocol;
pub mod stepper;
