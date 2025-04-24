/*! # Lightyear Native Inputs
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;
extern crate core;

pub(crate) mod action_state;

pub(crate) mod input_message;

pub mod plugin;


pub mod prelude {
    pub use crate::action_state::{ActionState, InputMarker};
    pub use crate::plugin::InputPlugin;
}