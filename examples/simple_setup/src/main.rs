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
use clap::{Parser, Subcommand, ValueEnum};
use core::time::Duration;
use lightyear::prelude::client::ClientPlugins;
use lightyear::prelude::server::ServerPlugins;

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

use lightyear::prelude::*;
use bevy::ecs::entity::MapEntities;
use serde::{Serialize, Deserialize};





// Add this system on Update on the client
fn handle_reconnect(mut commands: Commands, client: Single<Entity, (With<Client>, With<Connected>)>) -> Result {
    println!("Re-connecting client");
    commands.get_entity(*client)?.trigger(Disconnect).insert(Client::default()).trigger(Connect);
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let mut app = App::new();

    match cli.mode {
        Mode::Client => {
            app.add_plugins(DefaultPlugins);
            // add lightyear plugins
            app.add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // NOTE: the ProtocolPlugin must be added AFTER the Client/Server plugins,
            app.add_plugins(SharedPlugin);
            // add client-specific plugins
            app.add_plugins(client::ExampleClientPlugin);
        }
        Mode::Server => {
            app.add_plugins(DefaultPlugins);
            app.add_plugins(ServerPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // NOTE: the ProtocolPlugin must be added AFTER the Client/Server plugins
            app.add_plugins(SharedPlugin);
            app.add_plugins(server::ExampleServerPlugin);
        }
    }
    app.run();
}
