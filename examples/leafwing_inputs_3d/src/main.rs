#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
mod client;
mod protocol;
mod server;
mod shared;

use std::net::Ipv4Addr;
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
use lightyear::netcode::{ClientId, Key};
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
    Webtransport,
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    Server {
        /// If true, disable any rendering-related plugins
        #[arg(long, default_value = "false")]
        headless: bool,

        /// If true, enable bevy_inspector_egui
        #[arg(short, long, default_value = "false")]
        inspector: bool,

        /// The port to listen on
        #[arg(short, long, default_value_t = SERVER_PORT)]
        port: u16,

        /// Which transport to use
        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,

        /// If true, clients will predict everything (themselves, the ball, other clients)
        #[arg(long, default_value = "false")]
        predict: bool,
    },
    Client {
        /// If true, enable bevy_inspector_egui
        #[arg(short, long, default_value = "false")]
        inspector: bool,

        #[arg(short, long, default_value_t = 0)]
        client_id: u16,

        /// The port to listen on
        #[arg(long, default_value_t = CLIENT_PORT)]
        client_port: u16,

        #[arg(long, default_value_t = Ipv4Addr::LOCALHOST)]
        server_addr: Ipv4Addr,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        server_port: u16,

        /// Which transport to use
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
            transport,
            predict,
        } => {
            let server_plugin = MyServerPlugin {
                port,
                transport,
                predict_all: predict,
            };
            if !headless {
                app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            } else {
                app.add_plugins(MinimalPlugins);
            }
            if inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            app.add_plugins(server_plugin);
        }
        Cli::Client {
            inspector,
            client_id,
            client_port,
            server_addr,
            server_port,
            transport,
        } => {
            let client_plugin = MyClientPlugin {
                client_id: client_id as ClientId,
                client_port,
                server_addr,
                server_port,
                transport,
            };
            app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            // app.add_plugins(DefaultPlugins.set(LogPlugin {
            //     filter: "info,wgpu_core=warn,wgpu_hal=warn,mygame=debug".into(),
            //     level: bevy::log::Level::WARN,
            // }));
            if inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            app.add_plugins(client_plugin);
        }
    }
}
