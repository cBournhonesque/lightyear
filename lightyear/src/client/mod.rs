//! # Naia Client
//! A cross-platform client that can send/receive messages to/from a server, and
//! has a pool of in-scope Entities/Components that are synced with the
//! server.

#![deny(
    trivial_casts,
    trivial_numeric_casts,
    unstable_features,
    unused_import_braces
)]

mod client;
mod client_config;
mod command_history;
mod connection;
mod error;
mod events;
mod protocol;
mod tick;

pub use client::Client;
pub use client_config::ClientConfig;
pub use command_history::CommandHistory;
pub use error::NaiaClientError;
pub use events::{
    ConnectionEvent, DespawnEntityEvent, DisconnectionEvent, ErrorEvent, Events,
    InsertComponentEvent, MessageEvent, RejectionEvent, RemoveComponentEvent, SpawnEntityEvent,
    TickEvent, UpdateComponentEvent,
};

pub mod internal {
    pub use crate::client::connection::handshake_manager::{HandshakeManager, HandshakeState};
}
