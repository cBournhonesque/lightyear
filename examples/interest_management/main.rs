#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run --example bevy_cli server`
//! - `cargo run --example bevy_cli client`
//!
//! For webtransport:
//! - server: `cargo run --example interest_management --features webtransport -- server --transport web-transport`
mod client;
mod protocol;

#[cfg(not(target_family = "wasm"))]
mod server;
mod shared;

use std::net::{Ipv4Addr, SocketAddr};
#[cfg(target_family = "wasm")]
use web_sys;

use std::str::FromStr;

use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::DefaultPlugins;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt::format::FmtSpan;

use crate::client::MyClientPlugin;

#[cfg(not(target_family = "wasm"))]
use crate::server::MyServerPlugin;
#[cfg(not(target_family = "wasm"))]
use tokio;

use lightyear::prelude::Key;
#[cfg(target_family = "wasm")]
use wasm_bindgen_test::*;
#[cfg(target_family = "wasm")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        #[wasm_bindgen_test]
        fn test_client() {
            // let cli = Cli::parse();
            let cli = Cli::Client {
                client_id: 0,
                client_port: CLIENT_PORT,
                server_port: SERVER_PORT,
                transport: Transports::WebTransport,
            };

            let mut app = App::new();
            // app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            // app.add_plugins(DefaultPlugins.build());
            app.add_plugins(DefaultPlugins.set(LogPlugin {
                level: Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
            }));
            if let Cli::Client {
                client_id,
                client_port,
                server_port,
                transport,
            } = cli
            {
                let client_plugin =
                    client::create_plugin(client_id, client_port, server_port, transport);
                app.add_plugins(client_plugin);
            }
            // setup(&mut app, cli).await;

            app.run();
        }
        fn main() {}
    } else {
        #[tokio::main]
        async fn main() {
            let cli = Cli::parse();
            let mut app = App::new();
            // app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            // app.add_plugins(DefaultPlugins.build());
            app.add_plugins(DefaultPlugins.set(LogPlugin {
                level: Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
            }));
            setup(&mut app, cli).await;

            app.run();
        }

    }
}

pub const CLIENT_PORT: u16 = 6000;
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
        #[arg(short, long, default_value_t = SERVER_PORT)]
        port: u16,

        #[arg(short, long, value_enum, default_value_t = Transports::Udp)]
        transport: Transports,
    },
    Client {
        #[arg(short, long, default_value_t = 0)]
        client_id: u16,

        #[arg(long, default_value_t = CLIENT_PORT)]
        client_port: u16,

        #[arg(short, long, default_value_t = SERVER_PORT)]
        server_port: u16,

        #[cfg_attr(not(target_family = "wasm"), arg(short, long, value_enum, default_value_t = Transports::Udp))]
        #[cfg_attr(target_family = "wasm", arg(short, long, value_enum, default_value_t = Transports::WebTransport))]
        transport: Transports,
    },
}

async fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        #[cfg(not(target_family = "wasm"))]
        Cli::Server { port, transport } => {
            let server_plugin = server::create_plugin(port, transport).await;
            app.add_plugins(server_plugin);
        }
        Cli::Client {
            client_id,
            client_port,
            server_port,
            transport,
        } => {
            let client_plugin =
                client::create_plugin(client_id, client_port, server_port, transport);
            app.add_plugins(client_plugin);
        }
    }
}
