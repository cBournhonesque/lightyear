// Use the config module
mod config;

use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lightyear::prelude::client;
use lightyear::prelude::server;

use clap::Parser;
use strum::IntoEnumIterator;

use crate::config::*;
use core::net::SocketAddr;
use core::str::FromStr;
use lightyear::connection::client::Connect;
use lightyear::connection::server::Start;
use lightyear_examples_common::client::{ClientTransports, ExampleClient};
use lightyear_examples_common::client_renderer::ExampleClientRendererPlugin;
use lightyear_examples_common::server::{ExampleServer, ServerTransports};
use simple_box::client::ExampleClientPlugin as SimpleBoxClientPlugin;
use simple_box::protocol::ProtocolPlugin as SimpleBoxProtocolPlugin;
use simple_box::renderer::ExampleRendererPlugin as SimpleBoxRendererPlugin;
use simple_box::server::ExampleServerPlugin as SimpleBoxServerPlugin;
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::{exit, Command},
};

#[derive(Event, Debug)]
struct LaunchEvent;

/// Command-line arguments for launching directly without the UI.
/// Now only needs the path to the configuration file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Path to the RON configuration file.
    #[arg(long)]
    config_path: Option<PathBuf>,
}

fn main() {
    let cli_args = CliArgs::parse();

    // --- Direct Run Mode (using config file) ---
    if let Some(config_path) = cli_args.config_path {
        println!(
            "Detected direct run mode with config file: {:?}",
            config_path
        );

        // Load config from file
        let config = match fs::read_to_string(&config_path) {
            Ok(ron_data) => match ron::from_str::<LauncherConfig>(&ron_data) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("Failed to deserialize config from {:?}: {}", config_path, e);
                    exit(1);
                }
            },
            Err(e) => {
                eprintln!("Failed to read config file {:?}: {}", config_path, e);
                exit(1);
            }
        };
        fs::remove_file(&config_path).expect("Could not delete config file");

        println!("Loaded config for direct run: {:?}", config);

        // TODO: Determine asset path correctly if needed for direct run
        let asset_path = "../../assets".to_string(); // Placeholder

        // Build and run the appropriate app based on mode from config
        match config.mode {
            NetworkingMode::ClientOnly => {
                if config.client_id.is_some()
                    && config.server_addr.is_some()
                    && config.client_transport.is_some()
                {
                    println!("Direct launching {} ClientOnly...", config.example);
                    let mut client_app = build_client_app(config, asset_path);
                    client_app.run();
                } else {
                    eprintln!("Invalid config for ClientOnly mode (missing required fields).");
                    exit(1);
                }
            }
            NetworkingMode::ServerOnly => {
                if config.server_addr.is_some() && config.server_transport.is_some() {
                    println!("Direct launching {} ServerOnly...", config.example);
                    let mut server_app = build_server_app(config, asset_path);
                    server_app.run();
                } else {
                    eprintln!("Invalid config for ServerOnly mode (missing required fields).");
                    exit(1);
                }
            }
            NetworkingMode::HostServer => {
                if config.client_id.is_some()
                    && config.server_addr.is_some()
                    && config.client_transport.is_some()
                    && config.server_transport.is_some()
                {
                    println!("Direct launching {} HostServer...", config.example);
                    let mut host_server_app = build_host_server_app(config, asset_path);
                    host_server_app.run();
                } else {
                    eprintln!("Invalid config for HostServer mode (missing required fields).");
                    exit(1);
                }
            }
        }
        // Exit after direct run attempt
        exit(0);
    }

    // --- UI Mode (Default) ---
    info!("Starting launcher in UI mode...");
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Lightyear Example Launcher".into(),
                        ..default()
                    }),
                    ..default()
                })
                .disable::<LogPlugin>(),
        ) // Use Log settings from helper below
        .add_plugins(EguiPlugin::default())
        .init_resource::<LauncherConfig>() // Initialize with defaults for UI
        .add_plugins(LogPlugin {
            // Add common log settings here for UI app
            level: bevy::log::Level::INFO,
            filter: "wgpu=error,bevy_render=info,bevy_ecs=warn,lightyear=info".to_string(),
            ..default()
        })
        .add_systems(Startup, setup_system)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .run();
}

fn setup_system(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn ui_system(mut contexts: EguiContexts, mut config: ResMut<LauncherConfig>) -> Result {
    egui::CentralPanel::default().show(contexts.ctx_mut()?, |ui| {
        ui.heading("Lightyear Example Launcher");
        ui.separator();

        // === Mode Selection ===
        let mut mode_changed = false;
        let current_mode = config.mode;
        ui.horizontal(|ui| {
            ui.label("Networking Mode:");
            egui::ComboBox::from_id_salt("##NetworkingMode")
                .selected_text(current_mode.to_string())
                .show_ui(ui, |ui| {
                    for mode in NetworkingMode::iter() {
                         // Use selectable_value to directly modify config.mode
                        if ui.selectable_value(&mut config.mode, mode, mode.to_string()).changed() {
                             mode_changed = true;
                        }
                    }
                });
        });

        // Update optional configs if mode changed
        if mode_changed {
            let mode = config.mode;
             // Call helper function from config module
            config.update_defaults_for_mode(mode);
        }

        ui.separator();

        // === Example Selection ===
        ui.horizontal(|ui| {
            ui.label("Example:");
            egui::ComboBox::from_id_salt("##Example")
                .selected_text(config.example.to_string())
                .show_ui(ui, |ui| {
                    for example in Example::iter() {
                        ui.selectable_value(&mut config.example, example, example.to_string());
                    }
                });
        });
        ui.separator();


        // === Client Config (Conditional) ===
        if config.mode == NetworkingMode::ClientOnly || config.mode == NetworkingMode::HostServer {
            ui.group(|ui| {
                ui.heading("Client Settings");

                // Client ID
                ui.horizontal(|ui| {
                    ui.label("Client ID:");
                    // Provide a default empty string if None
                    let mut client_id_str = config.client_id.map_or(String::new(), |id| id.to_string());
                    // Use a unique ID for the TextEdit widget
                    if ui.add(egui::TextEdit::singleline(&mut client_id_str).id(egui::Id::new("client_id_input"))).changed() {
                        // Attempt to parse, update config only on success
                        if let Ok(id) = client_id_str.parse::<u64>() {
                            config.client_id = Some(id);
                        } else if client_id_str.is_empty() {
                            // Allow clearing the field
                            config.client_id = None;
                        } // else: keep the old value if parsing fails but not empty
                    }
                });


                // Server Address (for client connection)
                ui.horizontal(|ui| {
                    ui.label("Server Address:");
                    let mut server_addr_str = config.server_addr.map_or(String::new(), |addr| addr.to_string());
                     // Use a unique ID for the TextEdit widget
                    if ui.add(egui::TextEdit::singleline(&mut server_addr_str).id(egui::Id::new("client_server_addr"))).changed() {
                        if let Ok(addr) = SocketAddr::from_str(&server_addr_str) {
                            config.server_addr = Some(addr);
                        } else if server_addr_str.is_empty() {
                             config.server_addr = None;
                        }
                    }
                });

                // Client Transport
                ui.horizontal(|ui| {
                    ui.label("Client Transport:");
                    let current_transport_text = match &config.client_transport {
                        Some(ClientTransports::Udp) => "UDP",
                        Some(ClientTransports::WebTransport) => "WebTransport",
                        Some(transport) => "None",
                        None => "None",
                    };
                    egui::ComboBox::from_id_salt("##ClientTransport")
                        .selected_text(current_transport_text)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(matches!(config.client_transport, Some(ClientTransports::Udp)), "UDP").clicked() {
                                config.client_transport = Some(ClientTransports::Udp);
                            }
                            // Basic WebTransport selection - assumes native variant
                            if ui.selectable_label(matches!(config.client_transport, Some(ClientTransports::WebTransport)), "WebTransport").clicked() {
                                config.client_transport = Some(ClientTransports::WebTransport{
                                    #[cfg(target_family = "wasm")]
                                    certificate_digest: "".to_string()
                                });
                            }
                             if ui.selectable_label(config.client_transport.is_none(), "None").clicked() {
                                config.client_transport = None;
                            }
                        });
                });
                // TODO: Add UI for WebTransport certificate digest if target_family="wasm"
            });
        }

        // === Server Config (Conditional) ===
        if config.mode == NetworkingMode::ServerOnly || config.mode == NetworkingMode::HostServer {
             ui.group(|ui| {
                ui.heading("Server Settings");

                 // Server Bind Address
                 ui.horizontal(|ui| {
                    ui.label("Bind Address:");
                    // Default to empty string if None
                    let mut server_addr_str = config.server_addr.map_or(String::new(), |addr| addr.to_string());
                    let mut new_addr = None;
                     // Use a unique ID for the TextEdit widget
                    if ui.add(egui::TextEdit::singleline(&mut server_addr_str).id(egui::Id::new("server_bind_addr"))).changed() {
                        if let Ok(addr) = SocketAddr::from_str(&server_addr_str) {
                           new_addr = Some(addr);
                        } else if server_addr_str.is_empty() {
                            new_addr = None;
                        }
                    }
                    // Apply changes after the UI interaction
                    if let Some(addr) = new_addr { // Check if the address *input* changed to something valid or None
                        if let Some(ServerTransports::Udp { local_port }) = &mut config.server_transport {
                            *local_port = addr.port();
                        }
                        if let Some(ServerTransports::WebTransport { local_port, .. }) = &mut config.server_transport {
                            *local_port = addr.port();
                        }
                    } else if server_addr_str.is_empty() && ui.add(egui::TextEdit::singleline(&mut server_addr_str).id(egui::Id::new("server_bind_addr"))).changed() {
                         // Handle case where field was cleared
                         config.server_addr = None;
                    }
                });


                // Server Transport
                ui.horizontal(|ui| {
                    ui.label("Server Transport:");
                    let current_transport_text = match &config.server_transport {
                        Some(ServerTransports::Udp { .. }) => "UDP",
                        Some(ServerTransports::WebTransport { .. }) => "WebTransport",
                        Some(transport) => "None",
                        None => "None",
                    };
                     egui::ComboBox::from_id_salt("##ServerTransport")
                        .selected_text(current_transport_text)
                        .show_ui(ui, |ui| {
                            let port = config.server_addr.map_or(SERVER_PORT, |addr| addr.port());
                            // UDP Option
                            if ui.selectable_label(matches!(config.server_transport, Some(ServerTransports::Udp { .. })), "UDP").clicked() {
                                config.server_transport = Some(ServerTransports::Udp { local_port: port });
                            }
                            // WebTransport Option (Server)
                            if ui.selectable_label(matches!(config.server_transport, Some(ServerTransports::WebTransport { .. })), "WebTransport").clicked() {
                                warn!("WebTransport Server selected - Certificate handling is not implemented in UI!");
                                let port = config.server_addr.map_or(SERVER_PORT, |addr| addr.port());
                                config.server_transport = Some(ServerTransports::WebTransport {
                                    local_port: port,
                                    certificate: Default::default(),
                                });
                            }
                             if ui.selectable_label(config.server_transport.is_none(), "None").clicked() {
                                config.server_transport = None;
                            }
                        });
                });

                 // // Show/Edit UDP Port if UDP is selected
                 // if let Some(ServerTransports::Udp { local_port }) = &mut config.server_transport {
                 //     ui.horizontal(|ui| {
                 //         ui.label("UDP Port:");
                 //         let mut port_str = local_port.to_string();
                 //         let mut new_port_val: Option<u16> = None;
                 //         // TODO: make this non-editable! the port will h
                 //         // Use a unique ID for the TextEdit widget
                 //         if ui.add(egui::TextEdit::singleline(&mut port_str).id(egui::Id::new("server_udp_port"))).changed() {
                 //             if let Ok(port) = port_str.parse::<u16>() {
                 //                new_port_val = Some(port);
                 //             }
                 //         }
                 //     });
                 // }
                 // TODO: Add UI for WebTransport server certificate paths if needed

            }); // End Server Group
        }


        // TODO: Add UI for LinkConditioner settings
        // TODO: Add UI for other settings

        ui.separator();

        // Launch Button - Enable only if config is valid for the selected mode
        let launch_enabled = match config.mode {
            NetworkingMode::ClientOnly => config.client_id.is_some() && config.server_addr.is_some() && config.client_transport.is_some(),
            NetworkingMode::ServerOnly => config.server_addr.is_some() && config.server_transport.is_some(),
            NetworkingMode::HostServer => config.client_id.is_some() && config.server_addr.is_some() && config.client_transport.is_some() && config.server_transport.is_some(),
        };
        if ui.add_enabled(launch_enabled, egui::Button::new("Launch Example")).clicked() {
            info!("Launch button clicked! Config: {:?}", *config);
            launch_app(&config);
        }
        if !launch_enabled {
            ui.label("(Please complete configuration for the selected mode)");
        }
    });
    Ok(())
}

fn new_launcher_gui_app(title: String) -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .set(AssetPlugin {
                meta_check: bevy::asset::AssetMetaCheck::Never, // Workaround for wasm hot-reload
                ..default()
            })
            .set(LogPlugin {
                // Use common log settings
                level: bevy::log::Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=warn,lightyear=info".to_string(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window { title, ..default() }),
                ..default()
            }),
    );
    app
}

// Creates a Bevy app without GUI support (for ServerOnly)
fn new_launcher_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(), // Needed for server plugins sometimes
        LogPlugin {
            // Use common log settings
            level: bevy::log::Level::INFO,
            filter: "wgpu=error,bevy_render=info,bevy_ecs=warn,lightyear=info".to_string(),
            ..default()
        },
    ));
    app
}

fn build_server_app(config: LauncherConfig, _asset_path: String) -> App {
    info!(
        "Building Server App by adding plugins... Config: {:?}",
        config
    );
    let mut app = new_launcher_headless_app();

    // Extract required fields from config
    let server_transport = config
        .server_transport
        .expect("Server transport must be set for server mode");

    app.add_plugins((server::ServerPlugins {
        tick_duration: config.tick_duration,
    },));

    // Add example-specific server plugins (protocol might be added here or in ServerPlugins)
    add_example_server_plugins(&mut app, config.example);

    let server = app
        .world_mut()
        .spawn(ExampleServer {
            conditioner: None,
            transport: server_transport,
            shared: simple_box::shared::SHARED_SETTINGS,
        })
        .id();
    app.world_mut().trigger(Start { entity: server });

    app
}

// Generic build function for the client app
fn build_client_app(config: LauncherConfig, _asset_path: String) -> App {
    info!(
        "Building Client App by adding plugins... Config: {:?}",
        config
    );

    // Extract required fields from config
    let client_id = config
        .client_id
        .expect("Client ID must be set for client mode");
    let server_addr = config
        .server_addr
        .expect("Server address must be set for client mode");
    let client_transport = config
        .client_transport
        .expect("Client transport must be set for client mode");

    let mut app = new_launcher_gui_app(format!("{} Client {}", config.example, client_id));

    app.add_plugins((
        client::ClientPlugins {
            tick_duration: config.tick_duration,
        },
        ExampleClientRendererPlugin::new(String::new()), // Assuming this is still needed
    ));

    // Add example-specific client plugins (protocol might be added here or in ClientPlugins)
    add_example_client_plugins(&mut app, config.example);

    let client = app
        .world_mut()
        .spawn(ExampleClient {
            client_id,
            client_port: 0,
            server_addr,
            conditioner: None,
            transport: client_transport,
            shared: simple_box::shared::SHARED_SETTINGS,
        })
        .id();
    app.world_mut().trigger(Connect { entity: client });

    app
}

// Build function for HostServer mode (Client + Server in one App)
fn build_host_server_app(config: LauncherConfig, _asset_path: String) -> App {
    info!(
        "Building HostServer App by adding plugins... Config: {:?}",
        config
    );

    // Extract required fields (ensure they exist, checked by UI launch enable)
    let client_id = config.client_id.expect("Client ID missing for HostServer");
    let server_addr = config
        .server_addr
        .expect("Server address missing for HostServer");
    let client_transport_opt = config.client_transport; // Keep option for now
    let server_transport_opt = config.server_transport; // Keep option for now

    let mut app = new_launcher_gui_app(format!(
        "{} HostServer (Client {}, Server {})",
        config.example, client_id, server_addr
    ));

    app.add_plugins(client::ClientPlugins {
        tick_duration: config.tick_duration,
    });
    app.add_plugins(ExampleClientRendererPlugin::new(String::new()));
    app.add_plugins(server::ServerPlugins {
        tick_duration: config.tick_duration,
    });

    // --- Add Example Plugins (Common Protocol, Specific Client/Server Logic) ---
    add_example_server_plugins(&mut app, config.example); // Adds Protocol + Server Logic
    add_example_client_plugins(&mut app, config.example); // Adds Client Logic + Renderer

    // TODO: add client and server entities

    app
}

// Adds Protocol and Server-specific systems/components for an example
fn add_example_server_plugins(app: &mut App, example: Example) {
    match example {
        Example::SimpleBox => {
            app.add_plugins(SimpleBoxProtocolPlugin);
            app.add_plugins(SimpleBoxServerPlugin);
        }
        Example::Fps => {
            error!("FPS Example Server plugins not implemented!");
            // app.add_plugins(FpsProtocolPlugin);
            // app.add_plugins(FpsServerPlugin);
        }
    }
}

// Adds Client-specific systems/components and Renderer for an example
// Note: Protocol should ideally be added only once (e.g., in server or client setup)
// Or ensured that adding it twice is safe. Let's assume it's safe for now.
fn add_example_client_plugins(app: &mut App, example: Example) {
    match example {
        Example::SimpleBox => {
            // If ProtocolPlugin is already added by server plugins in HostServer,
            // adding it again might be redundant or cause issues depending on its implementation.
            // Consider adding ProtocolPlugin conditionally or refactoring it.
            // Let's assume adding it again is okay for now.
            app.add_plugins(SimpleBoxProtocolPlugin);
            app.add_plugins(SimpleBoxClientPlugin);
            app.add_plugins(SimpleBoxRendererPlugin);
        }
        Example::Fps => {
            error!("FPS Example Client plugins not implemented!");
            // app.add_plugins(FpsProtocolPlugin); // Maybe added already
            // app.add_plugins(FpsClientPlugin);
            // app.add_plugins(FpsRendererPlugin);
        }
    }
}

fn launch_app(config: &LauncherConfig) {
    let launch_config = config.clone(); // Clone the config to pass to the new process
    info!("Handling LaunchEvent for config: {:?}", launch_config);

    // 1. Serialize the configuration
    let config_data =
        match ron::ser::to_string_pretty(&launch_config, ron::ser::PrettyConfig::default()) {
            Ok(data) => data,
            Err(e) => {
                error!("Failed to serialize LauncherConfig to RON: {}", e);
                // Optionally show an error message in the UI here
                return;
            }
        };

    // 2. Create a temporary file
    let mut temp_file = match tempfile::Builder::new()
        .prefix("lightyear_launcher_cfg_")
        .suffix(".ron")
        .tempfile() // Creates file in OS temp dir
    {
        Ok(file) => file,
        Err(e) => {
            error!("Failed to create temporary config file: {}", e);
            return;
        }
    };

    // 3. Write the serialized config to the temporary file
    if let Err(e) = temp_file.write_all(config_data.as_bytes()) {
        error!("Failed to write config to temporary file: {}", e);
        return; // temp_file will be cleaned up on drop
    }

    // 4. Get the path of the temporary file.
    // We need to keep the NamedTempFile handle alive until the command is spawned,
    // or convert it to a TempPath which persists until it goes out of scope.
    let temp_path = temp_file.into_temp_path(); // Persists the file until temp_path drops

    // 5. Get the path to the currently running executable
    let current_exe = match env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            error!("Failed to get current executable path: {}", e);
            // temp_path will be cleaned up when this function returns
            return;
        }
    };

    // 6. Create the command to spawn the new process
    let mut command = Command::new(current_exe);
    command.arg("--config-path").arg(&*temp_path); // Pass the path to the config file

    info!(
        "Spawning process: {:?} --config-path {:?}",
        command.get_program(),
        &*temp_path
    );

    temp_path.keep().expect("Error in persisting temp file");

    // 7. Spawn the new process
    match command.spawn() {
        Ok(child) => {
            info!("Process spawned successfully with PID: {:?}", child.id());
            // Child process now runs independently with the config file
        }
        Err(e) => {
            error!("Failed to spawn process: {}", e);
            // Optionally show an error message in the UI
        }
    }
}
