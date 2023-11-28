#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run --example bevy_cli server`
//! - `cargo run --example bevy_cli client`
mod client;
mod protocol;
mod server;
mod shared;

use std::str::FromStr;

use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::DefaultPlugins;
use clap::Parser;
#[cfg(feature = "metrics")]
use metrics_exporter_prometheus;
use serde::{Deserialize, Serialize};

use crate::client::MyClientPlugin;
use crate::server::MyServerPlugin;
use lightyear_shared::netcode::{ClientId, Key};

fn main() {
    // Prepare tracing
    // let subscriber
    // #[cfg(feature = "metrics")]
    // Run a Prometheus scrape endpoint on 127.0.0.1:9000.
    // let _ = metrics_exporter_prometheus::PrometheusBuilder::new()
    //     .install()
    //     .expect("failed to install prometheus exporter");

    let cli = Cli::parse();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
    setup(&mut app, cli);

    app.run();
}

pub const PORT: u16 = 5000;
pub const PROTOCOL_ID: u64 = 0;

pub const KEY: Key = [0; 32];

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    Server {
        #[arg(short, long, default_value_t = PORT)]
        port: u16,
    },
    Client {
        #[arg(short, long, default_value_t = ClientId::default())]
        client_id: ClientId,

        #[arg(short, long, default_value_t = PORT)]
        server_port: u16,
    },
}

fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        Cli::Server { port } => {
            let server_plugin = MyServerPlugin { port };
            app.add_plugins(server_plugin);
        }
        Cli::Client {
            client_id,
            server_port,
        } => {
            let client_plugin = MyClientPlugin {
                client_id,
                server_port,
            };
            app.add_plugins(client_plugin);
        }
    }
}
