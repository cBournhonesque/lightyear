//! Utilities for building the Bevy app
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use core::str::FromStr;
use core::time::Duration;

use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;

use bevy::DefaultPlugins;
use bevy::diagnostic::DiagnosticsPlugin;
use bevy::state::app::StatesPlugin;
use clap::{Parser, Subcommand};

use crate::client::{ClientTransports, ExampleClient, connect};
#[cfg(all(feature = "gui", feature = "client"))]
use crate::client_renderer::ExampleClientRendererPlugin;
use crate::server::{ExampleServer, ServerTransports, WebTransportCertificateSettings, start};
#[cfg(all(feature = "gui", feature = "server"))]
use crate::server_renderer::ExampleServerRendererPlugin;
use crate::shared::{CLIENT_PORT, SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS};
#[cfg(feature = "gui")]
use bevy::window::PresentMode;
use lightyear::link::RecvLinkConditioner;
use lightyear::prelude::{Client, LinkConditionerConfig, LinkOf};

/// CLI options to create an [`App`]
#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub mode: Option<Mode>,
}

impl Cli {
    /// Get the client id from the CLI
    pub fn client_id(&self) -> Option<u64> {
        match &self.mode {
            #[cfg(feature = "client")]
            Some(Mode::Client { client_id }) => *client_id,
            #[cfg(all(feature = "client", feature = "server"))]
            Some(Mode::Separate { client_id }) => *client_id,
            #[cfg(all(feature = "client", feature = "server"))]
            Some(Mode::HostClient { client_id }) => *client_id,
            _ => None,
        }
    }

    pub fn build_app(&self, tick_duration: Duration, add_inspector: bool) -> App {
        match self.mode {
            #[cfg(feature = "client")]
            Some(Mode::Client { client_id }) => {
                let mut app = new_gui_app(add_inspector);
                app.add_plugins((
                    lightyear::prelude::client::ClientPlugins { tick_duration },
                    ExampleClientRendererPlugin::new(format!("Client {client_id:?}")),
                ));
                app
            }
            #[cfg(feature = "server")]
            Some(Mode::Server) => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "gui")] {
                        let mut app = new_gui_app(add_inspector);
                    } else {
                        let mut app = new_headless_app();
                    }
                }
                app.add_plugins((
                    lightyear::prelude::server::ServerPlugins { tick_duration },
                    ExampleServerRendererPlugin::new("Server".to_string()),
                ));
                app
            }
            #[cfg(all(feature = "client", feature = "server"))]
            Some(Mode::HostClient { client_id }) => {
                let mut app = new_gui_app(add_inspector);
                app.add_plugins((
                    lightyear::prelude::client::ClientPlugins { tick_duration },
                    lightyear::prelude::server::ServerPlugins { tick_duration },
                    ExampleClientRendererPlugin::new(format!("Host-Client {client_id:?}")),
                    ExampleServerRendererPlugin::new("Host-Server".to_string()),
                ));
                app
            }
            None => {
                panic!("Mode is required");
            }
            _ => {
                todo!()
            }
        }
    }

    pub fn spawn_connections(&self, app: &mut App) {
        match self.mode {
            #[cfg(feature = "client")]
            Some(Mode::Client { client_id }) => {
                let client = app
                    .world_mut()
                    .spawn(ExampleClient {
                        client_id: client_id.expect("You need to specify a client_id via `-c ID`"),
                        client_port: CLIENT_PORT,
                        server_addr: SERVER_ADDR,
                        conditioner: Some(RecvLinkConditioner::new(
                            LinkConditionerConfig::average_condition(),
                        )),
                        // transport: ClientTransports::Udp,
                        transport: ClientTransports::WebTransport,
                        shared: SHARED_SETTINGS,
                    })
                    .id();
                app.add_systems(Startup, connect);
            }
            #[cfg(feature = "server")]
            Some(Mode::Server) => {
                let server = app
                    .world_mut()
                    .spawn(ExampleServer {
                        conditioner: None,
                        // transport: ServerTransports::Udp {
                        //     local_port: SERVER_PORT,
                        // },
                        transport: ServerTransports::WebTransport {
                            local_port: SERVER_PORT,
                            certificate: WebTransportCertificateSettings::FromFile {
                                cert: "../../certificates/cert.pem".to_string(),
                                key: "../../certificates/key.pem".to_string(),
                            },
                        },
                        shared: SHARED_SETTINGS,
                    })
                    .id();
                app.add_systems(Startup, start);
            }
            #[cfg(all(feature = "client", feature = "server"))]
            Some(Mode::HostClient { client_id }) => {
                // Spawn the client and server connections here
                // This is where you would set up the client and server entities
                let server = app
                    .world_mut()
                    .spawn(ExampleServer {
                        conditioner: None,
                        // transport: ServerTransports::Udp {
                        //     local_port: SERVER_PORT,
                        // },
                        transport: ServerTransports::WebTransport {
                            local_port: SERVER_PORT,
                            certificate: WebTransportCertificateSettings::FromFile {
                                cert: "../../certificates/cert.pem".to_string(),
                                key: "../../certificates/key.pem".to_string(),
                            },
                        },
                        shared: SHARED_SETTINGS,
                    })
                    .id();

                let client = app
                    .world_mut()
                    .spawn((
                        Client::default(),
                        Name::new("HostClient"),
                        LinkOf { server },
                    ))
                    .id();
                // NOTE: it's ugly but i believe that you need to start the server before
                //  connecting the host-client for things to work properly
                app.add_systems(Startup, (start, connect).chain());
            }
            _ => {}
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    #[cfg(feature = "client")]
    /// Runs the app in client mode
    Client {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    #[cfg(feature = "server")]
    /// Runs the app in server mode
    Server,
    #[cfg(all(feature = "client", feature = "server"))]
    /// Creates two bevy apps: a client app and a server app.
    /// Data gets passed between the two via channels.
    Separate {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    #[cfg(all(feature = "client", feature = "server"))]
    /// Run the app in host-client mode.
    /// The client and the server will run inside the same app. The peer acts both as a client and a server.
    HostClient {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
}

impl Default for Mode {
    fn default() -> Self {
        cfg_if::cfg_if! {
            if #[cfg(all(feature = "client", feature = "server"))] {
                Mode::HostClient { client_id: None }
            } else if #[cfg(feature = "server")] {
                Mode::Server
            } else {
                Mode::Client { client_id: None }
            }
        }
    }
}

/// App that is Send.
/// Used as a convenient workaround to send an App to a separate thread,
/// if we know that the App doesn't contain NonSend resources.
struct SendApp(App);

unsafe impl Send for SendApp {}
impl SendApp {
    fn run(&mut self) {
        self.0.run();
    }
}

impl Default for Cli {
    fn default() -> Self {
        cli()
    }
}

/// Parse the CLI arguments.
/// `clap` doesn't run in wasm, so we simply run in Client mode with a random ClientId
pub fn cli() -> Cli {
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            let client_id = rand::random::<u64>();
            Cli {
                mode: Some(Mode::Client {
                    client_id: Some(client_id),
                })
            }
        } else {
            Cli::parse()
        }
    }
}

#[cfg(feature = "gui")]
pub fn window_plugin() -> WindowPlugin {
    WindowPlugin {
        primary_window: Some(Window {
            title: format!("Lightyear Example: {}", env!("CARGO_PKG_NAME")),
            resolution: (1024., 768.).into(),
            present_mode: PresentMode::AutoVsync,
            // set to true if we want to capture tab etc in wasm
            prevent_default_event_handling: true,
            ..Default::default()
        }),
        ..default()
    }
}

pub fn log_plugin() -> LogPlugin {
    LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=warn,bevy_time=warn".to_string(),
        ..default()
    }
}

#[cfg(feature = "gui")]
pub fn new_gui_app(add_inspector: bool) -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .set(AssetPlugin {
                // https://github.com/bevyengine/bevy/issues/10157
                meta_check: bevy::asset::AssetMetaCheck::Never,
                ..default()
            })
            .set(log_plugin())
            .set(window_plugin()),
    );
    if add_inspector {
        app.add_plugins(bevy_inspector_egui::bevy_egui::EguiPlugin {
            enable_multipass_for_primary_context: true,
        });
        app.add_plugins(bevy_inspector_egui::quick::WorldInspectorPlugin::new());
    }
    app
}

pub fn new_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        log_plugin(),
        StatesPlugin,
        DiagnosticsPlugin,
    ));
    app
}
