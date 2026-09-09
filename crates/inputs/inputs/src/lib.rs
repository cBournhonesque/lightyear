/*! # Lightyear Inputs (core)

Tick-indexed input buffering and input-message plumbing shared by all input
backends (native, leafwing, BEI).

This crate owns the [`InputBuffer`](input_buffer::InputBuffer) storage, the
[`ActionStateSequence`](input_message::ActionStateSequence) /
[`InputSnapshot`](input_message::InputSnapshot) traits that backends implement,
and the client/server systems that buffer local inputs, exchange
[`InputMessage`](input_message::InputMessage)s, and restore inputs for
simulation and rollback. Pick a backend crate for a concrete input type.
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
#[cfg(feature = "metrics")]
mod metric_handles;
pub mod plugin;
#[cfg(feature = "server")]
pub mod server;

pub(crate) const HISTORY_DEPTH: u32 = 20;

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
        pub use crate::client::{ClientInputPlugin, InputSystems};
    }
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::{
            InputRebroadcaster, InputSystems, InputValidationAppExt, ServerInputPlugin,
            authorize_controlled_targets,
        };
    }
}
