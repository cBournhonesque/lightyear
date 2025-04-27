use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin};
use lightyear::prelude::*;

use clap::Parser;
use strum::{Display, EnumIter, EnumString, IntoEnumIterator};

use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use std::{process::{exit, Command}, str::FromStr};
// Added Command and exit

use lightyear_examples_common_new::client::ClientTransports;
use lightyear_examples_common_new::client_renderer::ExampleClientRendererPlugin;
use lightyear_examples_common_new::server::ServerTransports;
use lightyear_examples_common_new::shared::SharedSettings;
use simple_box_new::client::ExampleClientPlugin as SimpleBoxClientPlugin;
use simple_box_new::protocol::ProtocolPlugin as SimpleBoxProtocolPlugin;
use simple_box_new::renderer::ExampleRendererPlugin as SimpleBoxRendererPlugin;
use simple_box_new::server::ExampleServerPlugin as SimpleBoxServerPlugin;


pub const FIXED_TIMESTEP_HZ: f64 = 64.0;
pub const SERVER_PORT: u16 = 5000;
/// 0 means that the OS will assign any available port
pub const CLIENT_PORT: u16 = 0;
pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SERVER_PORT);
pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0,
    ],
};

pub const SEND_INTERVAL: Duration = Duration::from_millis(100);

// TODO: Discover examples dynamically? For now, hardcode them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display)]
enum Example {
    SimpleBox,
    Fps,
    // Add other examples here
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display, EnumString)]
enum NetworkingMode {
    ClientOnly,
    ServerOnly,
    HostServer, // Server + Client in the same app
    // SeparateClientAndServer, // Srever + Client in separate apps but in the same process
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display, EnumString)]
enum TransportChoice {
    Udp,
    WebTransport,
    // Steam, // TODO: add steam
}

#[derive(Resource, Debug, Clone)]
struct LauncherConfig {
    example: Example,
    mode: NetworkingMode,
    // Use Options for conditional settings
    client_transport: Option<ClientTransports>,
    server_transport: Option<ServerTransports>,
    client_id: Option<u64>,
    server_addr: Option<SocketAddr>,
    tick_duration: Duration,
    // TODO: Add LinkConditioner settings
    // TODO: Add other settings like auth, encryption?
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let default_mode = NetworkingMode::HostServer;
        Self {
            example: Example::SimpleBox,
            mode: default_mode,
            // Set initial defaults based on HostServer mode
            client_transport: Some(ClientTransports::Udp),
            client_id: Some(0),
            server_addr: Some(SERVER_ADDR),
            server_transport: Some(ServerTransports::Udp { local_port: SERVER_PORT }),
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        }
    }
}

#[derive(Event, Debug)]
struct LaunchEvent;

/// Command-line arguments for launching directly without the UI.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// The networking mode to run in.
    #[arg(long)]
    run_mode: Option<NetworkingMode>,

    /// The example to run.
    #[arg(long)]
    example: Option<Example>, // Assuming Example derives FromStr or similar

    /// Client ID (required for ClientOnly, HostServer).
    #[arg(long)]
    client_id: Option<u64>,

    /// Server address (IP:port).
    #[arg(long)]
    server_addr: Option<SocketAddr>,

    /// Client transport type.
    #[arg(long)]
    client_transport: Option<TransportChoice>, // Use TransportChoice for simplicity

    /// Server transport type.
    #[arg(long)]
    server_transport: Option<TransportChoice>, // Use TransportChoice for simplicity

    /// Server port (used if server_addr is not provided for server).
    #[arg(long)]
    port: Option<u16>,
}

// Need FromStr for Example to be used in clap
impl FromStr for Example {
    type Err = String; // Or a more specific error type

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "simplebox" => Ok(Example::SimpleBox),
            "fps" => Ok(Example::Fps),
            _ => Err(format!("Unknown example: {}", s)),
        }
    }
}


fn main() {
    let cli_args = CliArgs::parse();

    // --- Direct Run Mode ---
    if let (Some(run_mode), Some(example)) = (cli_args.run_mode, cli_args.example) {
        info!("Detected direct run mode: {:?}, Example: {:?}", run_mode, example);

        // Construct LauncherConfig from CliArgs
        let server_addr = cli_args.server_addr.unwrap_or_else(|| {
            let port = cli_args.port.unwrap_or(SERVER_PORT);
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
        });

        let chosen_transport_cli = cli_args.client_transport.unwrap_or(TransportChoice::Udp);
        let client_transport = if cfg!(target_family = "wasm") {
            // WASM: Must use WebTransport, needs certificate_digest field.
            let digest = "".to_string(); // Placeholder for digest logic
            if chosen_transport_cli == TransportChoice::Udp {
                 warn!("UDP client transport selected via CLI, but target is wasm. Defaulting to WebTransport.");
            }
            warn!("WebTransport client direct launch on wasm requires certificate digest. Using empty digest as placeholder.");
            todo!();
        } else {
            // NATIVE: Can use UDP or WebTransport (without certificate_digest field).
            match chosen_transport_cli {
                TransportChoice::Udp => Some(ClientTransports::Udp),
                // Construct the variant that exists on native
                TransportChoice::WebTransport => Some(ClientTransports::WebTransport {}),
            }
        };

        let server_transport = match cli_args.server_transport.unwrap_or(TransportChoice::Udp) {
            TransportChoice::Udp => Some(ServerTransports::Udp { local_port: server_addr.port() }),
            TransportChoice::WebTransport => {
                warn!("WebTransport server direct launch not fully supported via CLI yet (certificate needed)");
                // TODO: How to handle certificates via CLI?
                None // Or default to UDP?
            }
        };

        let config = LauncherConfig {
            example,
            mode: run_mode,
            client_transport: if run_mode == NetworkingMode::ServerOnly { None } else { client_transport },
            server_transport: if run_mode == NetworkingMode::ClientOnly { None } else { server_transport },
            client_id: if run_mode == NetworkingMode::ServerOnly { None } else { cli_args.client_id }, // Use provided or None
            server_addr: Some(server_addr),
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        };

        // TODO: Determine asset path correctly if needed for direct run
        let asset_path = "../../assets".to_string(); // Placeholder

        // Build and run the appropriate app based on mode
        match config.mode {
            NetworkingMode::ClientOnly => {
                if config.client_id.is_some() && config.server_addr.is_some() && config.client_transport.is_some() {
                    info!("Direct launching {} ClientOnly...", config.example);
                    let mut client_app = build_client_app(config, asset_path);
                    client_app.run();
                } else {
                    error!("Cannot direct launch ClientOnly: Missing required arguments (client_id, server_addr, client_transport).");
                    exit(1);
                }
            }
            NetworkingMode::ServerOnly => {
                 if config.server_addr.is_some() && config.server_transport.is_some() {
                    info!("Direct launching {} ServerOnly...", config.example);
                    let mut server_app = build_server_app(config, asset_path);
                    server_app.run();
                } else {
                    error!("Cannot direct launch ServerOnly: Missing required arguments (server_addr, server_transport).");
                    exit(1);
                }
            }
            NetworkingMode::HostServer => {
                 if config.client_id.is_some() && config.server_addr.is_some() && config.client_transport.is_some() && config.server_transport.is_some() {
                    info!("Direct launching {} HostServer...", config.example);
                    let mut host_server_app = build_host_server_app(config, asset_path);
                    host_server_app.run();
                } else {
                    error!("Cannot direct launch HostServer: Missing required arguments.");
                    exit(1);
                }
            }
        }
        exit(0); // Exit after direct run completes
    }

    // --- UI Mode (Default) ---
    info!("Starting launcher in UI mode...");
    App::new()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
            primary_window: Some(Window {
                    title: "Lightyear Example Launcher".into(),
                    ..default()
                }),
                ..default()
            })
            .disable::<LogPlugin>())
        .add_plugins(EguiPlugin { enable_multipass_for_primary_context: false})
        .init_resource::<LauncherConfig>() // Initialize with defaults for UI
        .add_event::<LaunchEvent>()
        .add_systems(Update, ui_system)
        .add_systems(Update, launch_button_system.run_if(on_event::<LaunchEvent>))
        .run();
}

fn ui_system(
    mut contexts: EguiContexts,
    mut config: ResMut<LauncherConfig>,
    mut launch_event_writer: EventWriter<LaunchEvent>,
) {
    egui::CentralPanel::default().show(contexts.ctx_mut(), |ui| {
        ui.heading("Lightyear Example Launcher");
        ui.separator();

        // === Mode Selection ===
        let mut mode_changed = false;
        ui.horizontal(|ui| {
            ui.label("Networking Mode:");
            let current_mode = config.mode;
            egui::ComboBox::from_id_salt("##NetworkingMode") // Use unique label for ID
                .selected_text(current_mode.to_string())
                .show_ui(ui, |ui| {
                    for mode in NetworkingMode::iter() {
                        if ui.selectable_value(&mut config.mode, mode, mode.to_string()).changed() {
                            mode_changed = true;
                        }
                    }
                });
        });

        // Update optional configs if mode changed
        if mode_changed {
            let new_mode = config.mode;
            match new_mode {
                NetworkingMode::ClientOnly => {
                    if config.client_id.is_none() { config.client_id = Some(rand::random()); }
                    if config.server_addr.is_none() { config.server_addr = Some(SERVER_ADDR); }
                    if config.client_transport.is_none() { config.client_transport = Some(ClientTransports::Udp); }
                    config.server_transport = None;
                }
                NetworkingMode::ServerOnly => {
                    if config.server_addr.is_none() { config.server_addr = Some(SERVER_ADDR); } // Server needs bind address
                    if config.server_transport.is_none() { config.server_transport = Some(ServerTransports::Udp { local_port: SERVER_PORT }); }
                    config.client_id = None;
                    config.client_transport = None;
                    // Keep server_addr Some for binding
                }
                NetworkingMode::HostServer => {
                    if config.client_id.is_none() { config.client_id = Some(rand::random()); }
                    if config.server_addr.is_none() { config.server_addr = Some(SERVER_ADDR); }
                    if config.client_transport.is_none() { config.client_transport = Some(ClientTransports::Udp); }
                    if config.server_transport.is_none() { config.server_transport = Some(ServerTransports::Udp { local_port: SERVER_PORT }); }
                }
            }
        }

        ui.separator();

        // === Example Selection ===
        ui.horizontal(|ui| {
            ui.label("Example:");
            egui::ComboBox::from_id_salt("##Example") // Use unique label for ID
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
                    let mut client_id_str = config.client_id.map_or(String::new(), |id| id.to_string());
                    if ui.text_edit_singleline(&mut client_id_str).changed() {
                        if let Ok(id) = client_id_str.parse::<u64>() {
                            config.client_id = Some(id);
                        }
                    }
                });

                // Server Address
                ui.horizontal(|ui| {
                    ui.label("Server Address:");
                    let mut server_addr_str = config.server_addr.map_or(String::new(), |addr| addr.to_string());
                    if ui.add(egui::TextEdit::singleline(&mut server_addr_str).id(egui::Id::new("client_server_addr"))).changed() {
                        if let Ok(addr) = SocketAddr::from_str(&server_addr_str) {
                            config.server_addr = Some(addr);
                        }
                    }
                });

                // Client Transport
                ui.horizontal(|ui| {
                    ui.label("Client Transport:");
                    // Manually handle ClientTransports variants as it doesn't derive EnumIter
                    let current_transport_text = match config.client_transport {
                        Some(ClientTransports::Udp) => "UDP",
                        Some(ClientTransports::WebTransport { .. }) => "WebTransport",
                        // Add other variants if needed
                        None => "None",
                    };
                    egui::ComboBox::from_id_salt("##ClientTransport")
                        .selected_text(current_transport_text)
                        .show_ui(ui, |ui| {
                            // UDP Option
                            if ui.selectable_label(matches!(config.client_transport, Some(ClientTransports::Udp)), "UDP").clicked() {
                                config.client_transport = Some(ClientTransports::Udp);
                            }
                            // WebTransport Option
                            if ui.selectable_label(matches!(config.client_transport, Some(ClientTransports::WebTransport { .. })), "WebTransport").clicked() {
                                todo!();
                            }
                            // Add other variants here
                        });
                });
            });
        }

        // === Server Config (Conditional) ===
        if config.mode == NetworkingMode::ServerOnly || config.mode == NetworkingMode::HostServer {
             ui.group(|ui| {
                ui.heading("Server Settings");

                 // Server Address (for binding) - Reuse the same field as client's target address for simplicity now
                 ui.horizontal(|ui| {
                    ui.label("Bind Address:");
                    let mut server_addr_str = config.server_addr.map_or(String::new(), |addr| addr.to_string());
                    let mut new_addr = None; // Store potential new address
                    if ui.add(egui::TextEdit::singleline(&mut server_addr_str).id(egui::Id::new("server_bind_addr"))).changed() {
                        if let Ok(addr) = SocketAddr::from_str(&server_addr_str) {
                           new_addr = Some(addr); // Store if valid
                        }
                    }
                    // Apply changes after the UI interaction for this element
                    if let Some(addr) = new_addr {
                        config.server_addr = Some(addr); // Update address first
                        // Then update the port in the Udp transport if it exists
                        if let Some(ServerTransports::Udp { local_port }) = &mut config.server_transport {
                            *local_port = addr.port(); // Update transport port
                        }
                    }
                });


                // Server Transport
                ui.horizontal(|ui| {
                    ui.label("Server Transport:");
                    // Manually handle ServerTransports variants
                    let current_transport_text = match config.server_transport {
                        Some(ServerTransports::Udp { .. }) => "UDP",
                        Some(ServerTransports::WebTransport { .. }) => "WebTransport", // Added arm
                        // Add other variants if needed
                        None => "None",
                    };
                     egui::ComboBox::from_id_salt("##ServerTransport")
                        .selected_text(current_transport_text)
                        .show_ui(ui, |ui| {
                            // UDP Option
                            if ui.selectable_label(matches!(config.server_transport, Some(ServerTransports::Udp { .. })), "UDP").clicked() {
                                let port = config.server_addr.map_or(SERVER_PORT, |addr| addr.port());
                                config.server_transport = Some(ServerTransports::Udp { local_port: port });
                            }
                            // WebTransport Option (Server)
                            if ui.selectable_label(matches!(config.server_transport, Some(ServerTransports::WebTransport { .. })), "WebTransport").clicked() {
                                todo!();
                                // // TODO: Certificate handling needed for WebTransport Server!
                                // // For now, just set the variant. The build function will need adjustment.
                                // let port = config.server_addr.map_or(SERVER_PORT, |addr| addr.port()); // Get port
                                // config.server_transport = Some(ServerTransports::WebTransport {
                                //     local_port: port, // Added local_port
                                //     certificate: None // Placeholder cert
                                // });
                                // warn!("WebTransport Server selected - Certificate handling is not implemented!");
                            }
                            // Add other variants here
                        });
                });

                 // Show UDP Port if UDP is selected
                 let mut new_port = None; // Store the potential new port outside the closure
                 if let Some(ServerTransports::Udp { local_port }) = &config.server_transport { // Immutable borrow first
                     let current_port = *local_port; // Copy the port
                     ui.horizontal(|ui| {
                         ui.label("UDP Port:");
                         let mut port_str = current_port.to_string();
                         if ui.text_edit_singleline(&mut port_str).changed() {
                             if let Ok(port) = port_str.parse::<u16>() {
                                 new_port = Some(port); // Store the new port if valid
                             }
                         }
                     });
                 }

                 // Apply the change after the UI interaction
                 if let Some(port) = new_port {
                     if let Some(ServerTransports::Udp { local_port }) = &mut config.server_transport {
                         *local_port = port;
                     }
                     if let Some(addr) = &mut config.server_addr {
                         addr.set_port(port);
                     }
                 }

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
            launch_event_writer.send(LaunchEvent);
        }
        if !launch_enabled {
            ui.label("(Please complete configuration for the selected mode)");
        }
    });
}

// We need this because App implements Send but not Sync
struct SendApp(App);
unsafe impl Send for SendApp {}

impl SendApp {
    fn run(&mut self) {
        self.0.run();
    }
}

// --- App Creation Helpers (inspired by common_new/cli.rs) ---

// Creates a Bevy app with GUI support
fn new_launcher_gui_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .set(AssetPlugin {
                // Workaround for wasm hot-reload issue
                meta_check: bevy::asset::AssetMetaCheck::Never,
                ..default()
            })
            .set(LogPlugin { // Use common log settings
                level: bevy::log::Level::INFO,
                filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
                ..default()
            })
            // We don't set the WindowPlugin here, let the specific build function do it
            // if needed, so we can customize the title.
    );
    app
}

// Creates a Bevy app without GUI support (minimal plugins)
fn new_launcher_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(), // Needed for server plugins sometimes
        LogPlugin { // Use common log settings
            level: bevy::log::Level::INFO,
            filter: "wgpu=error,bevy_render=info,bevy_ecs=warn".to_string(),
            ..default()
        },
    ));
    app
}


fn build_server_app(config: LauncherConfig, _asset_path: String) -> App {
    info!("Building Server App by adding plugins... Config: {:?}", config);
    let mut app = new_launcher_headless_app();
    app.add_plugins(server::ServerPlugins {
         tick_duration: config.tick_duration,
    });

    // Add example-specific plugins via helper (this helper might need adjustment later)
    add_example_server_plugins(&mut app, config.example);
    app
}

// Helper to add only the server-specific logic plugins (not the protocol)
fn add_example_server_only_plugins(app: &mut App, example: Example) {
    match example {
        Example::SimpleBox => {
            app.add_plugins(SimpleBoxServerPlugin);
        }
        Example::Fps => {
            todo!();
        }
    }
}


// Generic build function for the client app
fn build_client_app(config: LauncherConfig, _asset_path: String) -> App {
    info!("Building Client App by adding plugins... Config: {:?}", config);
    let mut app = new_launcher_gui_app(); // Use GUI helper

    // Use expect() as the UI should ensure these are Some when launching client
    let client_id = config.client_id.expect("Client ID must be set for client mode");

    // Add Window plugin with custom title
    app.add_plugins(WindowPlugin {
        primary_window: Some(Window {
            title: format!("{} Client {}", config.example, client_id),
            ..default()
        }),
        ..default()
    });

    app.add_plugins((
        client::ClientPlugins {
            tick_duration: config.tick_duration,
        },
        ExampleClientRendererPlugin::new(String::new()),
    ));

    // Add example-specific plugins via helper
    add_example_client_plugins(&mut app, config.example);
    app
}

// Build function for HostServer mode (Client + Server in one App)
fn build_host_server_app(config: LauncherConfig, _asset_path: String) -> App {
    info!("Building HostServer App by adding plugins... Config: {:?}", config);
    let mut app = new_launcher_gui_app(); // Use GUI helper

    app.add_plugins((
        client::ClientPlugins {
            tick_duration: config.tick_duration,
        },
        server::ServerPlugins {
            tick_duration: config.tick_duration,
        },
        ExampleClientRendererPlugin::new(String::new()),
    ));

    // --- Window Title ---
     app.add_plugins(WindowPlugin {
        primary_window: Some(Window {
            title: format!("{} HostServer", config.example),
            ..default()
        }),
        ..default()
    });

    // --- Add Example Plugins (Client + Server) ---
    add_example_server_plugins(&mut app, config.example);
    add_example_client_plugins(&mut app, config.example);

    app
}


fn add_example_server_plugins(app: &mut App, example: Example) {
    match example {
        Example::SimpleBox => {
            app.add_plugins(SimpleBoxProtocolPlugin);
            app.add_plugins(SimpleBoxServerPlugin);
        }
        Example::Fps => {
            todo!()
        }
    }
}

fn add_example_client_plugins(app: &mut App, example: Example) {
     match example {
        Example::SimpleBox => {
            app.add_plugins(SimpleBoxProtocolPlugin);
            app.add_plugins(SimpleBoxClientPlugin);
            app.add_plugins(SimpleBoxRendererPlugin);
        }
        Example::Fps => {
            todo!();
        }
    }
}


// --- Launch Logic ---

fn launch_button_system(
    config: Res<LauncherConfig>,
    // We might need AssetServer later if examples load assets
    // asset_server: Res<AssetServer>,
) {
    let launch_config = config.clone();
    info!("Handling LaunchEvent for config: {:?}", launch_config);

    // TODO: Determine asset path correctly
    let asset_path = "../../assets".to_string(); // Placeholder

    match launch_config.example {
        Example::SimpleBox | Example::Fps => { // Apply launch logic to all examples for now
            match launch_config.mode {
                NetworkingMode::ClientOnly => {
                    // Ensure required config options are present for ClientOnly mode
                    if launch_config.client_id.is_some() && launch_config.server_addr.is_some() && launch_config.client_transport.is_some() {
                        info!("Launching {} ClientOnly...", launch_config.example);
                        let client_app = build_client_app(launch_config, asset_path); // Use generic build_client_app
                        let mut send_client_app = SendApp(client_app);
                        std::thread::spawn(move || send_client_app.run());
                    } else {
                        warn!("Cannot launch ClientOnly: Missing Client ID, Server Address, or Client Transport configuration.");
                    }
                }
                NetworkingMode::ServerOnly => {
                    // Ensure required config options are present for ServerOnly mode
                    if launch_config.server_addr.is_some() && launch_config.server_transport.is_some() {
                        info!("Launching {} ServerOnly...", launch_config.example);
                        let server_app = build_server_app(launch_config, asset_path); // Use generic build_server_app
                        let mut send_server_app = SendApp(server_app);
                        std::thread::spawn(move || send_server_app.run());
                    } else {
                        warn!("Cannot launch ServerOnly: Missing Server Address or Server Transport configuration.");
                    }
                }
                 NetworkingMode::HostServer => {
                    // Ensure required config options are present for HostServer mode
                    if launch_config.client_id.is_some() && launch_config.server_addr.is_some() && launch_config.client_transport.is_some() && launch_config.server_transport.is_some() {
                        info!("Launching {} HostServer (Client + Server in one app)...", launch_config.example);
                        // Build the combined HostServer app
                        let host_server_app = build_host_server_app(launch_config, asset_path);
                        let mut send_host_server_app = SendApp(host_server_app);
                        // Run the single app in a new thread
                        std::thread::spawn(move || send_host_server_app.run());
                    } else {
                         warn!("Cannot launch HostServer: Missing Client ID, Server Address, Client Transport, or Server Transport configuration.");
                    }
                }
            }
        }
        // Remove the separate Fps match arm as it's handled above now
        // Example::Fps => {
        //     warn!("FPS example launching not implemented yet!");
        //     // TODO: Implement FPS example launch logic
        // }
    }
}