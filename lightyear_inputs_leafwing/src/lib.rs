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