//! This example showcases how to use Lightyear with Bevy, to easily get replication along with prediction/interpolation working.
//!
//! There is a lot of setup code, but it's mostly to have the examples work in all possible configurations of transport.
//! (all transports are supported, as well as running the example in listen-server or host-server mode)
//!
//!
//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;

use bevy::asset::ron;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use lightyear::prelude::client::{InterpolationConfig, InterpolationDelay, NetConfig};
use serde::{Deserialize, Serialize};

use lightyear::prelude::TransportConfig;
use lightyear::shared::config::Mode;
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
    #[cfg(not(target_family = "wasm"))]
    /// Dedicated server
    Server,
    /// The program will act as a client. We will also launch the ServerPlugin in the same app
    /// so that a client can also act as host.
    Client {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
}

/// We parse the settings.ron file to read the settings, than create the apps and run them
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

/// This is the main function
/// The cli argument is used to determine if we are running as a client or a server (or listen-server)
/// Then we build the app and run it.
///
/// To build a lightyear app you will need to add either the [`client::ClientPlugin`] or [`server::ServerPlugin`]
/// They can be created by providing a [`client::ClientConfig`] or [`server::ServerConfig`] struct, along with a
/// shared protocol which defines the messages (Messages, Components, Inputs) that can be sent between client and server.
fn run(mut settings: Settings, cli: Cli) {
    match cli {
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
            if let Some(client_id) = client_id {
                settings.client.client_id = client_id;
            }
            let net_config = get_client_net_config(&settings);
            let mut app = combined_app(settings, net_config);
            app.run();
        }
    }
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
    let mut net_configs = get_server_net_configs(&settings);
    let extra_net_configs = extra_transport_configs.into_iter().map(|c| {
        build_server_netcode_config(settings.server.conditioner.as_ref(), &settings.shared, c)
    });
    net_configs.extend(extra_net_configs);
    let server_config = server::ServerConfig {
        shared: shared_config(Mode::Separate),
        net: net_configs,
        ..default()
    };
    app.add_plugins((
        server::ServerPlugin::new(server_config),
        ExampleServerPlugin,
        SharedPlugin,
    ));
    if settings.server.inspector {
        app.add_plugins(WorldInspectorPlugin::new());
    }
    app
}

/// An app that contains both the client and server plugins
#[cfg(not(target_family = "wasm"))]
fn combined_app(settings: Settings, client_net_config: client::NetConfig) -> App {
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
    let net_configs = get_host_server_net_configs(&settings);
    let server_config = server::ServerConfig {
        shared: shared_config(Mode::HostServer),
        net: net_configs,
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
        ExampleClientPlugin { settings },
    ));
    // shared plugin
    app.add_plugins(SharedPlugin);
    app
}
