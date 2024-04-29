//! Defines the Server bevy resource
//!
//! # Server
//! The server module contains all the code that is used to run the server.

pub mod config;

pub mod connection;

pub mod events;

pub mod input;

pub mod plugin;

pub mod room;

#[cfg_attr(docsrs, doc(cfg(feature = "leafwing")))]
#[cfg(feature = "leafwing")]
pub mod input_leafwing;
pub(crate) mod message;
pub(crate) mod prediction;

pub(crate) mod networking;
pub mod replication;
