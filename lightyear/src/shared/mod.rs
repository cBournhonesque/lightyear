//! Shared code between the server and client.

pub mod config;

pub mod events;

pub(crate) mod log;

pub mod ping;

pub mod plugin;

pub mod replication;

pub mod sets;

// TODO: refactor this out
pub mod systems;

pub mod tick_manager;

pub mod time_manager;
