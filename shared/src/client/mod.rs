#![allow(dead_code)]
#![allow(unused)]

pub use client::{Authentication, Client};
pub use config::ClientConfig;
pub use input::{InputConfig, InputSystemSet};
pub use ping_manager::PingConfig;
pub use plugin::{Plugin, PluginConfig};
pub use sync::{client_is_synced, SyncConfig};

pub mod client;
pub mod components;
pub mod config;
mod connection;
pub mod events;
pub mod input;
pub mod interpolation;
pub mod ping_manager;
mod plugin;
pub mod prediction;
pub mod sync;
mod systems;
