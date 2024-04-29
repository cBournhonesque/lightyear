#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::asset::ron;
use bevy::DefaultPlugins;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use lightyear::prelude::{Mode, TransportConfig};
use lightyear::prelude::client::{InterpolationConfig, InterpolationDelay, NetConfig};
use lightyear::prelude::server::PacketConfig;
use lightyear::shared::log::add_log_layer;
use lightyear::transport::LOCAL_SOCKET;

use crate::client::ExampleClientPlugin;
use crate::server::ExampleServerPlugin;
use crate::settings::*;
use crate::shared::{shared_config, SharedPlugin};

mod client;
mod protocol;
mod server;
mod settings;
mod shared;

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    /// We have the client and the server running inside the same app.
    /// The server will also act as a client.
    #[cfg(not(target_family = "wasm"))]
    HostServer {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    #[cfg(not(target_family = "wasm"))]
    /// We will create two apps: a client app and a server app.
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
        // ListenServer using a single app
        #[cfg(not(target_family = "wasm"))]
        Cli::HostServer { client_id } => {
            let client_net_config = NetConfig::Local {
                id: client_id.unwrap_or(settings.client.client_id),
            };
            let mut app = combined_app(settings, vec![], client_net_config);
            app.run();
        }
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
    app.add_plugins(DefaultPlugins.build().set(LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
        update_subscriber: Some(add_log_layer),
    }));
    if settings.client.inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }
    let client_config = client::ClientConfig {
        shared: shared_config(Mode::Separate),
        net: net_config,
        interpolation: InterpolationConfig {
            delay: InterpolationDelay::default().with_send_interval_ratio(2.0),
            ..default()
        },
        ..default()
    };
    app.add_plugins((
        client::ClientPlugin::new(client_config),
        ExampleClientPlugin,
        SharedPlugin,
    ));
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
        filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
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
    let server_config = server::ServerConfig {
        shared: shared_config(Mode::Separate),
        net: net_configs,
        packet: PacketConfig::default()
            // by default there is no bandwidth limit so we need to enable it
            .enable_bandwidth_cap()
            // we can set the max bandwidth to 56 KB/s
            .with_send_bandwidth_bytes_per_second_cap(1500),
        ..default()
    };
    app.add_plugins((
        server::ServerPlugin::new(server_config),
        ExampleServerPlugin,
        SharedPlugin,
    ));
    app
}

/// An app that contains both the client and server plugins
#[cfg(not(target_family = "wasm"))]
fn combined_app(
    settings: Settings,
    extra_transport_configs: Vec<TransportConfig>,
    client_net_config: client::NetConfig,
) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build().set(LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
        update_subscriber: Some(add_log_layer),
    }));
    if settings.client.inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }

    // server plugin
    let mut net_configs = get_server_net_configs(&settings);
    let extra_net_configs = extra_transport_configs.into_iter().map(|c| {
        build_server_netcode_config(settings.server.conditioner.as_ref(), &settings.shared, c)
    });
    net_configs.extend(extra_net_configs);
    let server_config = server::ServerConfig {
        shared: shared_config(Mode::HostServer),
        net: net_configs,
        packet: PacketConfig::default()
            // by default there is no bandwidth limit so we need to enable it
            .enable_bandwidth_cap()
            // we can set the max bandwidth to 56 KB/s
            .with_send_bandwidth_bytes_per_second_cap(1500),
        ..default()
    };
    app.add_plugins((
        server::ServerPlugin::new(server_config),
        ExampleServerPlugin,
    ));

    // client plugin
    let client_config = client::ClientConfig {
        shared: shared_config(Mode::HostServer),
        net: client_net_config,
        interpolation: InterpolationConfig {
            delay: InterpolationDelay::default().with_send_interval_ratio(2.0),
            ..default()
        },
        ..default()
    };
    app.add_plugins((
        client::ClientPlugin::new(client_config),
        ExampleClientPlugin,
    ));
    // shared plugin
    app.add_plugins(SharedPlugin);
    app
}
