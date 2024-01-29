#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
mod client;
mod protocol;

mod rivet;
mod server;
mod shared;

use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt::format::FmtSpan;

use crate::client::MyClientPlugin;
use crate::server::MyServerPlugin;
use lightyear::connection::netcode::{ClientId, Key};
use lightyear::prelude::TransportConfig;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut app = App::new();
    setup(&mut app, cli);

    app.run();
}

// Use a port of 0 to automatically select a port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_PORT: u16 = 5000;
pub const PROTOCOL_ID: u64 = 0;

pub const KEY: Key = [0; 32];

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Transports {
    Udp,
    WebTransport,
}

/// The type of the connection
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Connections {
    /// the default connection type: the netcode protocol
    Netcode,
    /// the connection will talk to the rivet service to find the server
    Rivet,
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    Server {
        #[arg(long, default_value = "false")]
        headless: bool,

        #[arg(short, long, default_value = "false")]
        inspector: bool,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        port: u16,

        #[arg(long, value_enum, default_value_t = Connections::Netcode)]
        connection: Connections,

        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,
    },
    Client {
        #[arg(short, long, default_value = "false")]
        inspector: bool,

        #[arg(short, long, default_value_t = 0)]
        client_id: u64,

        #[arg(long, default_value_t = CLIENT_PORT)]
        client_port: u16,

        #[arg(long, default_value_t = Ipv4Addr::LOCALHOST)]
        server_addr: Ipv4Addr,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        server_port: u16,

        #[arg(long, value_enum, default_value_t = Connections::Netcode)]
        connection: Connections,

        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,
    },
}

fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        Cli::Server {
            headless,
            inspector,
            port,
            connection,
            transport,
        } => {
            let server_plugin = server::create_plugin(headless, port, transport, connection);
            if !headless {
                app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            } else {
                app.add_plugins(MinimalPlugins);
            }
            if inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            server_plugin.build(app);
        }
        Cli::Client {
            inspector,
            client_id,
            client_port,
            server_addr,
            server_port,
            connection,
            transport,
        } => {
            let server_addr = SocketAddr::new(server_addr.into(), server_port);
            let client_plugin =
                client::create_plugin(client_id, client_port, server_addr, transport, connection);

            app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            if inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            client_plugin.build(app);
        }
    }
}
