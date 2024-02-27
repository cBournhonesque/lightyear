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

use async_compat::Compat;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::client::ClientPluginGroup;
#[cfg(not(target_family = "wasm"))]
use crate::server::ServerPluginGroup;
use lightyear::connection::netcode::{ClientId, Key};
use lightyear::prelude::TransportConfig;
use lightyear::shared::log::add_log_layer;

pub const PROTOCOL_ID: u64 = 0;

pub const KEY: Key = [0; 32];

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Transports {
    #[cfg(not(target_family = "wasm"))]
    Udp {
        local_port: u16,
    },
    WebTransport {
        local_port: u16,
    },
    WebSocket {
        local_port: u16,
    },
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    #[cfg(not(target_family = "wasm"))]
    Server,
    Client {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerSettings {
    /// If true, disable any rendering-related plugins
    headless: bool,

    /// If true, enable bevy_inspector_egui
    inspector: bool,

    /// Which transport to use
    transport: Vec<Transports>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ClientSettings {
    /// If true, enable bevy_inspector_egui
    inspector: bool,

    /// The client id
    client_id: u64,

    /// The client port to listen on
    client_port: u16,

    /// The ip address of the server
    server_addr: Ipv4Addr,

    /// The port of the server
    server_port: u16,

    /// Which transport to use
    transport: Transports,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub client: ClientSettings,
}

fn main() {
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            use wasm_bindgen::prelude::*;
            use web_sys::FileReader;
            let client_id = rand::random::<u64>();
            let cli = Cli::Client {
                client_id: Some(client_id)
            };
            let reader = FileReader::new().unwrap();
            let future = wasm_bindgen_futures::JsFuture::from(reader.read_as_text(&JsValue::from_str(file_url)).unwrap());
            // wasm_bindgen_futures::spawn_local(async move {
            //     let result = future.await;
            //     let text = result.unwrap().as_string().unwrap();
            //     web_sys::console::log_1(&text.into());
            // });
            let settings_str = IoTaskPool::get()
                .scope(|s| {
                    s.spawn(async move {
                        let result = future.await;
                        let text = result.unwrap().as_string().unwrap();
                        web_sys::console::log_1(&text.into());
                        text
                    });
                })
                .pop()
                .unwrap();
            dbg!(&settings_str);
        } else {
            let cli = Cli::parse();
            let settings_str = fs::read_to_string("assets/settings.ron").unwrap();
        }
    }
    let settings = ron::de::from_str::<Settings>(&settings_str).unwrap();
    let mut app = App::new();
    setup(&mut app, settings, cli);
    app.run();
}

fn setup(app: &mut App, settings: Settings, cli: Cli) {
    match cli {
        #[cfg(not(target_family = "wasm"))]
        Cli::Server => {
            let settings = settings.server;
            if !settings.headless {
                app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
            } else {
                app.add_plugins(MinimalPlugins);
            }
            app.add_plugins(LogPlugin {
                level: Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
                update_subscriber: Some(add_log_layer),
            });

            if settings.inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            // this is async because we need to load the certificate from io
            // we need async_compat because wtransport expects a tokio reactor
            let server_plugin_group = IoTaskPool::get()
                .scope(|s| {
                    s.spawn(Compat::new(async {
                        ServerPluginGroup::new(settings.transport).await
                    }));
                })
                .pop()
                .unwrap();
            app.add_plugins(server_plugin_group.build());
        }
        Cli::Client { client_id } => {
            let settings = settings.client;
            // NOTE: create the default plugins first so that the async task pools are initialized
            // use the default bevy logger for now
            // (the lightyear logger doesn't handle wasm)
            app.add_plugins(DefaultPlugins.build().set(LogPlugin {
                level: Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
                update_subscriber: Some(add_log_layer),
            }));
            if settings.inspector {
                app.add_plugins(WorldInspectorPlugin::new());
            }
            let server_addr = SocketAddr::new(settings.server_addr.into(), settings.server_port);
            let client_plugin_group = ClientPluginGroup::new(
                client_id.unwrap_or(settings.client_id),
                settings.client_port,
                server_addr,
                settings.transport,
            );
            app.add_plugins(client_plugin_group.build());
        }
    }
}
