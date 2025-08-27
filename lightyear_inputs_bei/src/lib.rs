//! # Lightyear Inputs BEI
//!
//! This crate provides an integration between `lightyear` and `bevy-enhanced-input`.
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

pub mod input_message;

mod marker;
mod plugin;
mod setup;

pub mod prelude {
    pub use crate::input_message::SnapshotBuffer;
    pub use crate::marker::InputMarker;
    pub use crate::plugin::InputPlugin;
    pub use crate::setup::{InputRegistryExt, ActionOfWrapper};
    pub use bevy_enhanced_input::prelude::*;
}
