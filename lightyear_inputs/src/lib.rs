/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "client")]
pub mod client;

pub mod config;
pub mod input_buffer;
pub mod input_message;
pub mod plugin;
#[cfg(feature = "server")]
pub mod server;

pub(crate) const HISTORY_DEPTH: u16 = 20;

/// Default channel to send inputs from client to server. This is a Sequenced Unreliable channel.
/// A marker struct for the default channel used to send inputs from client to server.
///
/// This channel is typically configured as a Sequenced Unreliable channel,
/// suitable for sending frequent, time-sensitive input data where occasional loss
/// is acceptable and out-of-order delivery is handled by sequencing.
pub struct InputChannel;

pub mod prelude {
    pub use crate::InputChannel;
    pub use crate::config::InputConfig;
    pub use crate::input_buffer::InputBuffer;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{ClientInputPlugin, InputSet};
    }
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::{InputRebroadcaster, InputSet, ServerInputPlugin, ServerInputConfig};
    }
}
