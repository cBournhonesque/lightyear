#![allow(dead_code)]
#![allow(unused)]

pub use client::{Authentication, Client};
pub use config::ClientConfig;
pub use input::InputSystemSet;
pub use ping_manager::PingConfig;
pub use plugin::{Plugin, PluginConfig};

pub(crate) mod client;
mod config;
mod connection;
mod events;
mod interpolation;
mod ping_manager;
mod plugin;

// #[cfg(feature = "prediction")]
mod input;
pub mod prediction;
mod sync;
// mod tick_manager;
