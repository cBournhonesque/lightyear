#![allow(dead_code)]
#![allow(unused)]

pub use config::{NetcodeConfig, ServerConfig};
pub use ping_manager::PingConfig;
pub use plugin::{Plugin, PluginConfig};
pub use server::Server;

mod config;
mod connection;
mod events;
mod input;
pub(crate) mod io;
mod ping_manager;
mod plugin;
mod server;
// mod tick_manager;
