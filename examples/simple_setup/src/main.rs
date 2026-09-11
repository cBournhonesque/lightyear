//! This minimal example showcases how to setup the lightyear plugins.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

mod client;
mod server;
mod shared;

use crate::shared::{SharedPlugin, FIXED_TIMESTEP_HZ};
use bevy::prelude::*;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use core::time::Duration;
use lightyear::connection::server::Start;
use lightyear::prelude::client::ClientPlugins;
use lightyear::prelude::server::ServerPlugins;
use lightyear::prelude::server::ServerUdpIo;
use lightyear::prelude::*;

/// CLI options to create an [`App`]
#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub mode: Mode,
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    Client,
    Server,
}

fn main() {
    let cli = Cli::parse();
    let mut app = App::new();

    app.add_plugins(DefaultPlugins);
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
    // ProtocolPlugin must be added after the Client/Server plugins.
    app.add_plugins(SharedPlugin);
    app.world_mut().spawn(Camera2d);

    match cli.mode {
        Mode::Client => {
            app.add_plugins(client::ExampleClientPlugin);
        }
        Mode::Server => {
            app.add_plugins(server::ExampleServerPlugin);
        }
    }
    app.run();
}
