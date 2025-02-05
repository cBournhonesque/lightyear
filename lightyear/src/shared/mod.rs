//! Shared code between the server and client.

pub mod config;

pub mod events;

pub mod ping;

pub mod plugin;

pub mod replication;

pub mod sets;

pub mod tick_manager;

pub mod identity;
pub mod input;
pub(crate) mod message;
pub mod run_conditions;
pub mod time_manager;
