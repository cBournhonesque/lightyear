//! # Lightyear Inputs BEI
//!
//! This crate provides an integration between `lightyear` and `bevy-enhanced-input`.
#![no_std]

#[cfg(feature = "std")]
extern crate alloc;
extern crate core;

pub mod input_message;

mod marker;
mod plugin;
pub mod registry;

pub mod prelude {
    pub use crate::marker::InputMarker;
    pub use crate::plugin::InputPlugin;
    pub use crate::registry::{InputRegistry, InputRegistryExt};
    pub use bevy_enhanced_input::prelude::*;
}
