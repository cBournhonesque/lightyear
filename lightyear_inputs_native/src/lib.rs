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

pub mod prelude {
    pub use crate::action_state::{ActionState, InputMarker};
    pub use crate::plugin::InputPlugin;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::ClientInputPlugin;
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerInputPlugin;
    }
}