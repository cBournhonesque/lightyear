/*! Modules related to the client
*/

pub mod components;

pub mod config;

pub mod connection;

pub mod events;

pub mod input;

pub mod interpolation;

pub mod plugin;

pub mod prediction;

pub mod sync;

mod diagnostics;
mod easings;

pub(crate) mod io;
pub(crate) mod message;
pub(crate) mod networking;
pub mod replication;

#[cfg(target_family = "wasm")]
mod web;
