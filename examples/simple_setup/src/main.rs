//! This minimal example showcases how to setup the lightyear plugins.
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

mod automation;
mod client;
mod server;
mod shared;

use crate::automation::{ClientStartupConfig, ServerStartupConfig};
use crate::shared::{SharedPlugin, FIXED_TIMESTEP_HZ};
use bevy::prelude::*;
use clap::{Parser, Subcommand, ValueEnum};
use core::time::Duration;
use lightyear::connection::server::Start;
use lightyear::prelude::client::ClientPlugins;
use lightyear::prelude::server::ServerPlugins;
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerUdpIo,
};
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
    HostClient {
        #[arg(short, long, default_value_t = 0)]
        client_id: u64,
    },
}

fn main() {
    let cli = Cli::parse();
    let mut app = automation::build_base_app();

    match cli.mode {
        Mode::Client => {
            // add lightyear plugins
            app.add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // NOTE: the ProtocolPlugin must be added AFTER the Client/Server plugins,
            app.add_plugins(SharedPlugin);
            app.insert_resource(ClientStartupConfig {
                client_id: 0,
                host_server: None,
            });
            // add client-specific plugins
            app.add_plugins(client::ExampleClientPlugin);
        }
        Mode::Server => {
            app.add_plugins(ServerPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            // NOTE: the ProtocolPlugin must be added AFTER the Client/Server plugins
            app.add_plugins(SharedPlugin);
            app.add_plugins(server::ExampleServerPlugin);
        }
        Mode::HostClient { client_id } => {
            app.add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            app.add_plugins(ServerPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
            });
            app.add_plugins(SharedPlugin);
            app.add_plugins(server::ExampleServerPlugin);
            app.insert_resource(ServerStartupConfig { auto_spawn: false });
            let server = app
                .world_mut()
                .spawn((
                    NetcodeServer::new(ServerNetcodeConfig::default()),
                    LocalAddr(shared::SERVER_ADDR),
                    ServerUdpIo::default(),
                ))
                .id();
            app.world_mut().trigger(Start { entity: server });
            app.insert_resource(ClientStartupConfig {
                client_id,
                host_server: Some(server),
            });
            app.add_plugins(client::ExampleClientPlugin);
        }
    }
    app.run();
}
