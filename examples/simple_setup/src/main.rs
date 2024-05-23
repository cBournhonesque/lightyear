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

use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use bevy::prelude::*;
use clap::Parser;

#[derive(Parser, PartialEq, Debug)]
pub enum Cli {
    /// The program will act as server
    Server,
    /// The program will act as a client
    Client,
}

fn main() {
    let cli = Cli::parse();
    let mut app = App::new();
    match cli {
        Cli::Server => app.add_plugins(ExampleServerPlugin),
        Cli::Client => app.add_plugins(ExampleClientPlugin),
    };
    app.run();
}
