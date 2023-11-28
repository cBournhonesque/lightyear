//! Shared code between the server and client.

/// Configuration that has to be the same between the server and the client.
pub mod config;

/// Bevy events that will be emitted upon receiving network messages
pub mod events;

/// Log plugin that also potentially emits metrics to Prometheus
pub(crate) mod log;

/// Module to handle sending ping/pong messages and compute connection statistics (rtt, jitter, etc.)
pub mod ping;

/// Bevy [`bevy::prelude::Plugin`] used by both the server and the client
pub mod plugin;

/// Module to handle replicating entities and components from server to client
pub mod replication;

/// Bevy [`SystemSet`](bevy::prelude::SystemSet) that are shared between the server and client
pub mod sets;

// TODO: refactor this out
/// Bevy [`System`](bevy::prelude::System) that are shared between the server and client
pub mod systems;

/// Module to handle the [`tick_manager::Tick`], a sequence number incremented at each [`bevy::prelude::FixedUpdate`] schedule run
pub mod tick_manager;

/// Module to handle tracking time
pub mod time_manager;
