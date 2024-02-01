#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
mod client;
mod protocol;
#[cfg(not(target_family = "wasm"))]
mod server;
mod shared;

use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::client::ClientPluginGroup;
#[cfg(not(target_family = "wasm"))]
use crate::server::ServerPluginGroup;
use lightyear::connection::netcode::{ClientId, Key};
use lightyear::prelude::TransportConfig;

// Use a port of 0 to automatically select a port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_PORT: u16 = 5000;
pub const PROTOCOL_ID: u64 = 0;

pub const KEY: Key = [0; 32];

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Transports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport,
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    #[cfg(not(target_family = "wasm"))]
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
        #[arg(short, long, value_enum, default_value_t = Transports::WebTransport)]
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
        client_id: u64,

        /// The port to listen on
        #[arg(long, default_value_t = CLIENT_PORT)]
        client_port: u16,

        #[arg(long, default_value_t = Ipv4Addr::LOCALHOST)]
        server_addr: Ipv4Addr,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        server_port: u16,

        /// Which transport to use
        #[arg(short, long, value_enum, default_value_t = Transports::WebTransport)]
        transport: Transports,
    },
}

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        fn main() {
            // NOTE: clap argument parsing does not work on WASM
            let client_id = rand::random::<u64>();
            let cli = Cli::Client {
                inspector: false,
                client_id,
                client_port: CLIENT_PORT,
                server_addr: Ipv4Addr::LOCALHOST,
                server_port: SERVER_PORT,
                transport: Transports::WebTransport,
            };
            let mut app = App::new();
            setup_client(&mut app, cli);
            app.run();
        }
    } else {
        #[tokio::main]
        async fn main() {
            let cli = Cli::parse();
            let mut app = App::new();
            setup(&mut app, cli).await;
            app.run();
        }
    }
}

async fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        #[cfg(not(target_family = "wasm"))]
        Cli::Server {
            headless,
            inspector,
            port,
            transport,
            predict,
        } => {
            let server_plugin_group = ServerPluginGroup::new(port, transport).await;
            if !headless {
                app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            } else {
                app.add_plugins(MinimalPlugins);
            }
            if inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            app.add_plugins(server_plugin_group.build());
        }
        Cli::Client { .. } => {
            setup_client(app, cli);
        }
    }
}

fn setup_client(app: &mut App, cli: Cli) {
    let Cli::Client {
        inspector,
        client_id,
        client_port,
        server_addr,
        server_port,
        transport,
    } = cli
    else {
        return;
    };
    // NOTE: create the default plugins first so that the async task pools are initialized
    // use the default bevy logger for now
    // (the lightyear logger doesn't handle wasm)
    app.add_plugins(DefaultPlugins.set(LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
    }));
    if inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }
    let server_addr = SocketAddr::new(server_addr.into(), server_port);
    let client_plugin_group =
        ClientPluginGroup::new(client_id, client_port, server_addr, transport);
    app.add_plugins(client_plugin_group.build());
}
