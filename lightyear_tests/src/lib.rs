#![allow(unused_must_use)]
/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

#[cfg(test)]
#[cfg(feature = "test_utils")]
mod client_server;
pub mod protocol;
pub mod stepper;

#[cfg(test)]
mod host_server;

#[cfg(test)]
mod multi_server;
