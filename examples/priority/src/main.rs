#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

//! Run with
//! - `cargo run -- server`
//! - `cargo run -- client -c 1`
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use async_compat::Compat;
use bevy::asset::ron;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy::DefaultPlugins;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

use lightyear::connection::netcode::ClientId;
use lightyear::prelude::server::Certificate;
use lightyear::prelude::TransportConfig;
use lightyear::shared::log::add_log_layer;
use lightyear::transport::LOCAL_SOCKET;

use crate::client::ClientPluginGroup;
#[cfg(not(target_family = "wasm"))]
use crate::server::ServerPluginGroup;

mod client;
mod protocol;

#[cfg(not(target_family = "wasm"))]
mod server;
mod shared;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport {
        certificate_digest: String,
    },
    WebSocket,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ServerTransports {
    Udp { local_port: u16 },
    WebTransport { local_port: u16 },
    WebSocket { local_port: u16 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerSettings {
    /// If true, disable any rendering-related plugins
    headless: bool,

    /// If true, enable bevy_inspector_egui
    inspector: bool,

    /// Which transport to use
    transport: Vec<ServerTransports>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    transport: ClientTransports,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
pub struct SharedSettings {
    /// An id to identify the protocol version
    protocol_id: u64,

    /// a 32-byte array to authenticate via the Netcode.io protocol
    private_key: [u8; 32],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub client: ClientSettings,
    pub shared: SharedSettings,
}

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    #[cfg(not(target_family = "wasm"))]
    /// The program will act both as a server and as a client
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
            let transport_config = TransportConfig::LocalChannel {
                recv: from_server_recv,
                send: to_server_send,
            };
            // when communicating via channels, we need to use the address `LOCAL_SOCKET` for the server
            let mut client_app =
                client_app(settings.clone(), LOCAL_SOCKET, client_id, transport_config);

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
            let transport_config = get_client_transport_config(settings.client.clone());
            let mut app = client_app(settings, server_addr, client_id, transport_config);
            app.run();
        }
    }
}

/// Build the client app
fn client_app(
    settings: Settings,
    server_addr: SocketAddr,
    client_id: Option<ClientId>,
    transport_config: TransportConfig,
) -> App {
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
    let client_plugin_group = ClientPluginGroup::new(
        // use the cli-provided client id if it exists, otherwise use the settings client id
        client_id.unwrap_or(settings.client.client_id),
        server_addr,
        transport_config,
        settings.shared,
    );
    app.add_plugins(client_plugin_group.build());
    app
}

/// Build the server app
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
    let mut transport_configs = get_server_transport_configs(settings.server.transport);
    transport_configs.extend(extra_transport_configs);
    let server_plugin_group = ServerPluginGroup::new(transport_configs, settings.shared);
    app.add_plugins(server_plugin_group.build());
    app
}

/// Parse the server transport settings into a list of `TransportConfig` that are used to configure the lightyear server
fn get_server_transport_configs(settings: Vec<ServerTransports>) -> Vec<TransportConfig> {
    settings
        .iter()
        .map(|t| match t {
            ServerTransports::Udp { local_port } => TransportConfig::UdpSocket(SocketAddr::new(
                Ipv4Addr::UNSPECIFIED.into(),
                *local_port,
            )),
            ServerTransports::WebTransport { local_port } => {
                // this is async because we need to load the certificate from io
                // we need async_compat because wtransport expects a tokio reactor
                let certificate = IoTaskPool::get()
                    .scope(|s| {
                        s.spawn(Compat::new(async {
                            Certificate::load("../certificates/cert.pem", "../certificates/key.pem")
                                .await
                                .unwrap()
                        }));
                    })
                    .pop()
                    .unwrap();
                let digest = &certificate.hashes()[0].to_string().replace(":", "");
                println!("Generated self-signed certificate with digest: {}", digest);
                TransportConfig::WebTransportServer {
                    server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
                    certificate,
                }
            }
            ServerTransports::WebSocket { local_port } => TransportConfig::WebSocketServer {
                server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
            },
        })
        .collect()
}

/// Parse the client transport settings into a `TransportConfig` that is used to configure the lightyear client
fn get_client_transport_config(settings: ClientSettings) -> TransportConfig {
    let server_addr = SocketAddr::new(settings.server_addr.into(), settings.server_port);
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), settings.client_port);
    match settings.transport {
        #[cfg(not(target_family = "wasm"))]
        ClientTransports::Udp => TransportConfig::UdpSocket(client_addr),
        ClientTransports::WebTransport { certificate_digest } => {
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                #[cfg(target_family = "wasm")]
                certificate_digest,
            }
        }
        ClientTransports::WebSocket => TransportConfig::WebSocketClient { server_addr },
    }
}
