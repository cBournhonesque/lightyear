//! Shared code between the server and client.

pub mod config;

pub mod events;

pub mod log;

pub mod ping;

pub mod plugin;

pub mod replication;

pub mod sets;

pub mod tick_manager;

pub mod input;

#[cfg(feature = "leafwing")]
pub mod input_leafwing;
pub(crate) mod message;
pub mod run_conditions;
pub mod time_manager;
