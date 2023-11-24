#![allow(dead_code)]
#![allow(unused)]

pub use client::{Authentication, Client};
pub use config::ClientConfig;
pub use input::InputSystemSet;
pub use ping_manager::PingConfig;
pub use plugin::{Plugin, PluginConfig};
pub use sync::{client_is_synced, SyncConfig};

pub mod client;
pub mod components;
pub mod config;
mod connection;
mod events;
mod input;
pub mod interpolation;
mod ping_manager;
mod plugin;
pub mod prediction;
mod sync;
mod systems;
