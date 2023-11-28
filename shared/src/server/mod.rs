//! # Server
//! The server module contains all the code that is used to run the server.
#![allow(dead_code)]
#![allow(unused)]

/// Defines server-specific configuration options
pub mod config;

/// Wrapper around [`crate::connection::Connection`] that adds server-specific functionality
mod connection;

/// Wrapper around [`crate::connection::events::ConnectionEvents`] that adds server-specific functionality
pub mod events;

/// Handles client-generated inputs
mod input;

/// Defines the server bevy plugin
pub mod plugin;

/// Defines the server bevy resource
pub mod resource;

/// Defines the server bevy systems and run conditions
mod systems;
