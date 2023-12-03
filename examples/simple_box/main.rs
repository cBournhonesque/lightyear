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
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt::format::FmtSpan;

use crate::client::MyClientPlugin;
use crate::server::MyServerPlugin;
use lightyear::netcode::{ClientId, Key};
use lightyear::prelude::TransportConfig;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
    setup(&mut app, cli);

    app.run();
}

pub const CLIENT_PORT: u16 = 6000;
pub const SERVER_PORT: u16 = 5000;
pub const PROTOCOL_ID: u64 = 0;

pub const KEY: Key = [0; 32];

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Transports {
    Udp,
    Webtransport,
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    Server {
        #[arg(short, long, default_value_t = SERVER_PORT)]
        port: u16,

        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,
    },
    Client {
        #[arg(short, long, default_value_t = ClientId::default())]
        client_id: ClientId,

        #[arg(long, default_value_t = CLIENT_PORT)]
        client_port: u16,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        server_port: u16,

        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,
    },
}

fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        Cli::Server { port, transport } => {
            let server_plugin = MyServerPlugin { port, transport };
            app.add_plugins(server_plugin);
        }
        Cli::Client {
            client_id,
            client_port,
            server_port,
            transport,
        } => {
            let client_plugin = MyClientPlugin {
                client_id,
                client_port,
                server_port,
                transport,
            };
            app.add_plugins(client_plugin);
        }
    }
}
