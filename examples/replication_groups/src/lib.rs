//! This example demonstrates how to use replication groups to control entity replication.
//! It includes modules for the protocol, client, server, renderer, and shared logic.
pub mod protocol;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "gui")]
pub mod renderer;

pub mod shared;
