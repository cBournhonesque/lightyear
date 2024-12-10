//! Utilities for building the Bevy app
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::asset::ron;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;

use bevy::state::app::StatesPlugin;
use bevy::DefaultPlugins;
use clap::{Parser, ValueEnum};
use lightyear::prelude::client::ClientConfig;
use lightyear::prelude::*;
use lightyear::prelude::{client, server};
use lightyear::server::config::ServerConfig;
use lightyear::shared::log::add_log_layer;
use lightyear::transport::LOCAL_SOCKET;
use serde::{Deserialize, Serialize};

use crate::settings::*;
use crate::shared::{shared_config, REPLICATION_INTERVAL};

#[cfg(feature = "gui")]
use crate::renderer::ExampleRendererPlugin;

/// CLI options to create an [`App`]
#[derive(Parser, PartialEq, Debug)]
pub enum Cli {
    /// We have the client and the server running inside the same app.
    /// The server will also act as a client. (i.e. one client acts as the 'host')
    #[cfg(all(feature = "client", feature = "server"))]
    HostServer {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    /// We will create two apps: a client app and a server app.
    /// Data gets passed between the two via channels.
    #[cfg(all(feature = "client", feature = "server"))]
    ClientAndServer {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
    /// Dedicated server
    #[cfg(feature = "server")]
    Server,
    /// The program will act as a client
    #[cfg(feature = "client")]
    Client {
        #[arg(short, long, default_value = None)]
        client_id: Option<u64>,
    },
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
            Cli::Client {
                client_id: Some(client_id)
            }
        } else {
            Cli::parse()
        }
    }
}

/// Apps that will be returned from the `build_apps` function
///
/// The configs are also included so that the user can modify them if needed, before running the app.
pub enum Apps {
    /// A single app that contains only the ClientPlugins
    Client { app: App, config: ClientConfig },
    /// A single app that contains only the ServerPlugins
    Server { app: App, config: ServerConfig },
    /// Two apps (Client and Server) that will run in separate threads
    ClientAndServer {
        client_app: App,
        client_config: ClientConfig,
        server_app: App,
        server_config: ServerConfig,
    },
    /// A single app that contains both the Client and Server plugins
    HostServer {
        app: App,
        client_config: ClientConfig,
        server_config: ServerConfig,
    },
}

impl Apps {
    /// Build the apps with the given settings and CLI options.
    pub fn new(settings: Settings, cli: Cli, name: String) -> Self {
        match cli {
            #[cfg(all(feature = "client", feature = "server"))]
            Cli::HostServer { client_id } => {
                let client_net_config = client::NetConfig::Local {
                    id: client_id.unwrap_or(settings.client.client_id),
                };
                let (mut app, client_config, server_config) =
                    combined_app(settings, vec![], client_net_config);
                app.add_plugins(ExampleRendererPlugin::new(name));
                Apps::HostServer {
                    app,
                    client_config,
                    server_config,
                }
            }
            #[cfg(all(feature = "client", feature = "server"))]
            Cli::ClientAndServer { client_id } => {
                // we will communicate between the client and server apps via channels
                let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
                let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
                let transport_config = client::ClientTransport::LocalChannel {
                    recv: from_server_recv,
                    send: to_server_send,
                };

                // create client app
                let net_config = build_client_netcode_config(
                    client_id.unwrap_or(settings.client.client_id),
                    // when communicating via channels, we need to use the address `LOCAL_SOCKET` for the server
                    LOCAL_SOCKET,
                    settings.client.conditioner.as_ref(),
                    &settings.shared,
                    transport_config,
                );
                let (mut client_app, client_config) = client_app(settings.clone(), net_config);
                client_app.add_plugins(ExampleRendererPlugin::new(name));

                // create server app, which will be headless when we have client app in same process
                let extra_transport_configs = vec![server::ServerTransport::Channels {
                    // even if we communicate via channels, we need to provide a socket address for the client
                    channels: vec![(LOCAL_SOCKET, to_server_recv, from_server_send)],
                }];
                let (server_app, server_config) = server_app(settings, extra_transport_configs);
                Apps::ClientAndServer {
                    client_app,
                    client_config,
                    server_app,
                    server_config,
                }
            }
            #[cfg(feature = "server")]
            Cli::Server => {
                #[allow(unused_mut)]
                let (mut app, config) = server_app(settings, vec![]);
                #[cfg(feature = "gui")]
                app.add_plugins(ExampleRendererPlugin::new(name));
                Apps::Server { app, config }
            }
            #[cfg(feature = "client")]
            Cli::Client { client_id } => {
                let server_addr = SocketAddr::new(
                    settings.client.server_addr.into(),
                    settings.client.server_port,
                );
                // use the cli-provided client id if it exists, otherwise use the settings client id
                let client_id = client_id.unwrap_or(settings.client.client_id);
                let net_config = get_client_net_config(&settings, client_id);
                let (mut app, config) = client_app(settings, net_config);
                app.add_plugins(ExampleRendererPlugin::new(name));
                Apps::Client { app, config }
            }
        }
    }

    /// Set the `server_replication_send_interval` on client and server apps.
    /// Use to overwrite the default [`SharedConfig`] value in the settings file.
    pub fn with_server_replication_send_interval(mut self, replication_interval: Duration) -> Self {
        self.update_lightyear_client_config(|cc: &mut ClientConfig| {
            cc.shared.server_replication_send_interval = replication_interval
        });
        self.update_lightyear_server_config(|sc: &mut ServerConfig| {
            // the server replication currently needs to be overwritten in both places...
            sc.shared.server_replication_send_interval = replication_interval;
            sc.replication.send_interval = replication_interval;
        });
        self
    }

    /// Add the lightyear [`ClientPlugins`] and [`ServerPlugins`] plugin groups to the app.
    ///
    /// This can be called after any modifications to the [`ClientConfig`] and [`ServerConfig`]
    /// have been applied.
    pub fn add_lightyear_plugins(&mut self) -> &mut Self {
        match self {
            Apps::Client { app, config } => {
                app.add_plugins(client::ClientPlugins {
                    config: config.clone(),
                });
            }
            Apps::Server { app, config } => {
                app.add_plugins(server::ServerPlugins {
                    config: config.clone(),
                });
            }
            Apps::ClientAndServer {
                client_app,
                server_app,
                client_config,
                server_config,
            } => {
                client_app.add_plugins(client::ClientPlugins {
                    config: client_config.clone(),
                });
                server_app.add_plugins(server::ServerPlugins {
                    config: server_config.clone(),
                });
            }
            Apps::HostServer {
                app,
                client_config,
                server_config,
            } => {
                // TODO: currently we need ServerPlugins to run first, because it adds the
                //  SharedPlugins. not ideal
                app.add_plugins(client::ClientPlugins {
                    config: client_config.clone(),
                });
                app.add_plugins(server::ServerPlugins {
                    config: server_config.clone(),
                });
            }
        }
        self.add_bevygap_plugins();
        self
    }

    /// Adds bevygap plugins, if enabled with feature flags
    fn add_bevygap_plugins(&mut self) {
        // inject bevygap plugins
        #[cfg(any(feature = "bevygap_client", feature = "bevygap_server"))]
        {
            println!("🌐 Bevygap features are enabled");
            self.add_user_shared_plugin(crate::bevygap_shared::BevygapSharedExtensionPlugin);
        }
        #[cfg(feature = "bevygap_client")]
        {
            self.add_user_client_plugin(bevygap_client_plugin::prelude::BevygapClientPlugin);
        }
        #[cfg(feature = "bevygap_server")]
        {
            self.add_user_server_plugin(bevygap_server_plugin::prelude::BevygapServerPlugin);
        }
    }

    /// Adds plugin to the client app
    pub fn add_user_client_plugin(&mut self, client_plugin: impl Plugin) -> &mut Self {
        match self {
            Apps::Client { app, .. } => {
                app.add_plugins(client_plugin);
            }
            Apps::ClientAndServer { client_app, .. } => {
                client_app.add_plugins(client_plugin);
            }
            Apps::HostServer { app, .. } => {
                app.add_plugins(client_plugin);
            }
            Apps::Server { .. } => {}
        }
        self
    }

    /// Adds plugin to the server app
    pub fn add_user_server_plugin(&mut self, server_plugin: impl Plugin) -> &mut Self {
        match self {
            Apps::Client { .. } => {}
            Apps::ClientAndServer { server_app, .. } => {
                server_app.add_plugins(server_plugin);
            }
            Apps::HostServer { app, .. } => {
                app.add_plugins(server_plugin);
            }
            Apps::Server { app, .. } => {
                app.add_plugins(server_plugin);
            }
        }
        self
    }

    /// Adds plugin to both the server and client apps, if present
    pub fn add_user_shared_plugin(&mut self, shared_plugin: impl Plugin + Clone) -> &mut Self {
        match self {
            Apps::Client { app, config } => {
                app.add_plugins(shared_plugin);
            }
            Apps::ClientAndServer {
                server_app,
                client_app,
                ..
            } => {
                server_app.add_plugins(shared_plugin.clone());
                client_app.add_plugins(shared_plugin);
            }
            Apps::HostServer { app, .. } => {
                app.add_plugins(shared_plugin);
            }
            Apps::Server { app, .. } => {
                app.add_plugins(shared_plugin);
            }
        }
        self
    }

    /// Adds to the client app, and the server app if in standalone server mode with the cargo "gui" feature.
    /// Won't add renderer to server app if a client app also present.
    pub fn add_user_renderer_plugin(&mut self, renderer_plugin: impl Plugin) -> &mut Self {
        match self {
            Apps::Client { app, config } => {
                app.add_plugins(renderer_plugin);
            }
            Apps::ClientAndServer {
                server_app,
                client_app,
                ..
            } => {
                client_app.add_plugins(renderer_plugin);
            }
            Apps::HostServer { app, .. } => {
                app.add_plugins(renderer_plugin);
            }
            Apps::Server { app, .. } => {
                #[cfg(feature = "gui")]
                app.add_plugins(renderer_plugin);
            }
        }
        self
    }

    /// Add the client, server, and shared user-provided plugins to the app
    pub fn add_user_plugins(
        &mut self,
        client_plugin: impl Plugin,
        server_plugin: impl Plugin,
        shared_plugin: impl Plugin + Clone,
    ) -> &mut Self {
        self.add_user_shared_plugin(shared_plugin)
            .add_user_client_plugin(client_plugin)
            .add_user_server_plugin(server_plugin)
    }

    /// Apply a function to update the [`ClientConfig`]
    pub fn update_lightyear_client_config(
        &mut self,
        f: impl FnOnce(&mut ClientConfig),
    ) -> &mut Self {
        match self {
            Apps::Client { config, .. } => {
                f(config);
            }
            Apps::Server { config, .. } => {}
            Apps::ClientAndServer { client_config, .. } => {
                f(client_config);
            }
            Apps::HostServer { client_config, .. } => {
                f(client_config);
            }
        }
        self
    }

    /// Apply a function to update the [`ServerConfig`]
    pub fn update_lightyear_server_config(
        &mut self,
        f: impl FnOnce(&mut ServerConfig),
    ) -> &mut Self {
        match self {
            Apps::Client { config, .. } => {}
            Apps::Server { config, .. } => {
                f(config);
            }
            Apps::ClientAndServer { server_config, .. } => {
                f(server_config);
            }
            Apps::HostServer { server_config, .. } => {
                f(server_config);
            }
        }
        self
    }

    /// Start running the apps.
    pub fn run(self) {
        match self {
            Apps::Client { mut app, .. } => {
                app.run();
            }
            Apps::Server { mut app, .. } => {
                app.run();
            }
            Apps::ClientAndServer {
                mut client_app,
                server_app,
                ..
            } => {
                let mut send_app = SendApp(server_app);
                std::thread::spawn(move || send_app.run());
                client_app.run();
            }
            Apps::HostServer { mut app, .. } => {
                app.run();
            }
        }
    }
}

fn window_plugin() -> WindowPlugin {
    WindowPlugin {
        primary_window: Some(Window {
            title: format!("Lightyear Example: {}", env!("CARGO_PKG_NAME")),
            resolution: (1024., 768.).into(),
            present_mode: bevy::window::PresentMode::AutoVsync,
            // Tells wasm to resize the window according to the available canvas
            fit_canvas_to_parent: true,
            // set to true if we want to capture tab etc in wasm
            prevent_default_event_handling: true,
            ..Default::default()
        }),
        ..default()
    }
}

fn log_plugin() -> LogPlugin {
    LogPlugin {
        level: Level::INFO,
        filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
        ..default()
    }
}

fn new_gui_app() -> App {
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
    app
}

fn new_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, log_plugin(), StatesPlugin, HierarchyPlugin));
    app
}

/// Build the client app with the `ClientPlugins` added.
/// Takes in a `net_config` parameter so that we configure the network transport.
#[cfg(feature = "client")]
fn client_app(settings: Settings, net_config: client::NetConfig) -> (App, ClientConfig) {
    let app = new_gui_app();

    let client_config = ClientConfig {
        shared: shared_config(Mode::Separate),
        net: net_config,
        replication: ReplicationConfig {
            send_interval: REPLICATION_INTERVAL,
            ..default()
        },
        ..default()
    };
    (app, client_config)
}

/// Build the server app with the `ServerPlugins` added.
#[cfg(feature = "server")]
fn server_app(
    settings: Settings,
    extra_transport_configs: Vec<server::ServerTransport>,
) -> (App, ServerConfig) {
    use cfg_if::cfg_if;

    info!("server_app. gui={}", cfg!(feature = "gui"));
    // If there's a client app, the server needs to be headless.
    // Winit doesn't support two event loops in the same thread.
    cfg_if::cfg_if! {
        if #[cfg(feature = "client")] {
            let app = new_headless_app();
        } else if #[cfg(feature = "gui")] {
            let app = new_gui_app();
        } else {
            let app = new_headless_app();
        }
    }
    // configure the network configuration
    let mut net_configs = get_server_net_configs(&settings);
    let extra_net_configs = extra_transport_configs.into_iter().map(|c| {
        build_server_netcode_config(settings.server.conditioner.as_ref(), &settings.shared, c)
    });
    net_configs.extend(extra_net_configs);
    let server_config = ServerConfig {
        shared: shared_config(Mode::Separate),
        net: net_configs,
        replication: ReplicationConfig {
            send_interval: REPLICATION_INTERVAL,
            ..default()
        },
        ..default()
    };
    (app, server_config)
}

/// An `App` that contains both the client and server plugins
#[cfg(all(feature = "client", feature = "server"))]
fn combined_app(
    settings: Settings,
    extra_transport_configs: Vec<server::ServerTransport>,
    client_net_config: client::NetConfig,
) -> (App, ClientConfig, ServerConfig) {
    let app = new_gui_app();
    // server config
    let mut net_configs = get_server_net_configs(&settings);
    let extra_net_configs = extra_transport_configs.into_iter().map(|c| {
        build_server_netcode_config(settings.server.conditioner.as_ref(), &settings.shared, c)
    });
    net_configs.extend(extra_net_configs);
    let server_config = ServerConfig {
        shared: shared_config(Mode::HostServer),
        net: net_configs,
        replication: ReplicationConfig {
            send_interval: REPLICATION_INTERVAL,
            ..default()
        },
        ..default()
    };

    // client config
    let client_config = ClientConfig {
        shared: shared_config(Mode::HostServer),
        net: client_net_config,
        ..default()
    };
    (app, client_config, server_config)
}
