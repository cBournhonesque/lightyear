#![allow(dead_code)]
#![allow(unused)]

pub use client::{Authentication, Client};
pub use config::ClientConfig;
pub use input::InputSystemSet;
pub use ping_manager::PingConfig;
pub use plugin::{Plugin, PluginConfig};
pub use sync::SyncConfig;

pub mod client;
mod config;
mod connection;
mod events;
mod ping_manager;
mod plugin;

// #[cfg(feature = "prediction")]
mod input;
pub mod interpolation;
pub mod prediction;
mod sent_packet_store;
mod sync;
// mod tick_manager;
