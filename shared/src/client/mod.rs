//! # Client
//! //! The client module contains all the code that is used to run the client.
#![allow(dead_code)]
#![allow(unused)]

/// Defines components that are used for the client-side prediction and interpolation
pub mod components;

/// Defines client-specific configuration options
pub mod config;

/// Wrapper around [`crate::connection::Connection`] that adds client-specific functionality
pub mod connection;

/// Wrapper around [`crate::connection::events::ConnectionEvents`] that adds client-specific functionality
pub mod events;

/// Handles client-generated inputs
pub mod input;

/// Handles interpolation of entities between server updates
pub mod interpolation;

/// Defines the client bevy plugin
pub mod plugin;

/// Handles client-side prediction
pub mod prediction;

/// Defines the client bevy resource
pub mod resource;

/// Handles syncing the time between the client and the server
pub mod sync;

/// Defines the client bevy systems and run conditions
pub mod systems;
