#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::asset::ron;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use lightyear::prelude::TransportConfig;
use lightyear::shared::log::add_log_layer;
use lightyear::transport::LOCAL_SOCKET;

use crate::client::ClientPluginGroup;
use crate::server::ServerPluginGroup;
use crate::settings::*;

mod client;
mod protocol;
mod server;
mod settings;
mod shared;

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    #[cfg(not(target_family = "wasm"))]
    /// The program will act both as a server and as a client.
    /// Data gets passed between the two via channels.
    ListenServer {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    #[cfg(not(target_family = "wasm"))]
    /// Dedicated server
    Server,
    /// The program will act as a client
    Client {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
}

fn main() {
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            let client_id = rand::random::<u64>();
            let cli = Cli::Client {
                client_id: Some(client_id)
            };
        } else {
            let cli = Cli::parse();
        }
    }
    let settings_str = include_str!("../assets/settings.ron");
    let settings = ron::de::from_str::<Settings>(settings_str).unwrap();
    run(settings, cli);
}

fn run(settings: Settings, cli: Cli) {
    match cli {
        #[cfg(not(target_family = "wasm"))]
        Cli::ListenServer { client_id } => {
            // create client app
            let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
            let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
            // we will communicate between the client and server apps via channels
            let transport_config = TransportConfig::LocalChannel {
                recv: from_server_recv,
                send: to_server_send,
            };
            let net_config = build_client_netcode_config(
                client_id.unwrap_or(settings.client.client_id),
                // when communicating via channels, we need to use the address `LOCAL_SOCKET` for the server
                LOCAL_SOCKET,
                settings.client.conditioner.as_ref(),
                &settings.shared,
                transport_config,
            );
            let mut client_app = client_app(settings.clone(), net_config);

            // create server app
            let extra_transport_configs = vec![TransportConfig::Channels {
                // even if we communicate via channels, we need to provide a socket address for the client
                channels: vec![(LOCAL_SOCKET, to_server_recv, from_server_send)],
            }];
            let mut server_app = server_app(settings, extra_transport_configs);

            // run both the client and server apps
            std::thread::spawn(move || server_app.run());
            client_app.run();
        }
        #[cfg(not(target_family = "wasm"))]
        Cli::Server => {
            let mut app = server_app(settings, vec![]);
            app.run();
        }
        Cli::Client { client_id } => {
            let server_addr = SocketAddr::new(
                settings.client.server_addr.into(),
                settings.client.server_port,
            );
            // use the cli-provided client id if it exists, otherwise use the settings client id
            let client_id = client_id.unwrap_or(settings.client.client_id);
            let net_config = get_client_net_config(&settings, client_id);
            let mut app = client_app(settings, net_config);
            app.run();
        }
    }
}

/// Build the client app
fn client_app(settings: Settings, net_config: client::NetConfig) -> App {
    let mut app = App::new();
    // NOTE: create the default plugins first so that the async task pools are initialized
    // use the default bevy logger for now
    // (the lightyear logger doesn't handle wasm)
    app.add_plugins(DefaultPlugins.build().set(LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
        update_subscriber: Some(add_log_layer),
    }));
    if settings.client.inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }
    let client_plugin_group = ClientPluginGroup::new(net_config);
    app.add_plugins(client_plugin_group.build());
    app
}

/// Build the server app
#[cfg(not(target_family = "wasm"))]
fn server_app(settings: Settings, extra_transport_configs: Vec<TransportConfig>) -> App {
    let mut app = App::new();
    if !settings.server.headless {
        app.add_plugins(DefaultPlugins.build().disable::<LogPlugin>());
    } else {
        app.add_plugins(MinimalPlugins);
    }
    app.add_plugins(LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
        update_subscriber: Some(add_log_layer),
    });

    if settings.server.inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }
    let mut net_configs = get_server_net_configs(&settings);
    let extra_net_configs = extra_transport_configs.into_iter().map(|c| {
        build_server_netcode_config(settings.server.conditioner.as_ref(), &settings.shared, c)
    });
    net_configs.extend(extra_net_configs);
    let server_plugin_group = ServerPluginGroup::new(net_configs);
    app.add_plugins(server_plugin_group.build());
    app
}
