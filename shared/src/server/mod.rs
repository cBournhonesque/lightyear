#![allow(dead_code)]
#![allow(unused)]

pub use config::{NetcodeConfig, ServerConfig};
pub use ping_manager::PingConfig;
pub use plugin::{PluginConfig, ServerPlugin};
pub use server::Server;

pub mod config;
mod connection;
pub mod events;
mod input;
pub(crate) mod io;
pub mod ping_manager;
mod plugin;
mod server;

mod systems;
// mod tick_manager;
