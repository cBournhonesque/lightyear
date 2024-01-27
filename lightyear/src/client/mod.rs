/*! The Client bevy resource
*/

pub mod components;

pub mod config;

pub mod connection;

pub mod events;

pub mod input;

pub mod interpolation;

pub mod plugin;

pub mod prediction;

pub mod resource;

pub mod sync;

mod diagnostics;
mod easings;
#[cfg(feature = "leafwing")]
pub mod input_leafwing;
pub(crate) mod message;
pub mod systems;
