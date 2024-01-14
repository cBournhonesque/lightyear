//! Defines the Server bevy resource
//!
//! # Server
//! The server module contains all the code that is used to run the server.

pub mod config;

pub mod connection;

pub mod events;

mod input;

pub mod plugin;

pub mod resource;

pub mod room;

#[cfg(feature = "leafwing")]
pub mod input_leafwing;
mod systems;
