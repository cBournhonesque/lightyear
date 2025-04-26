//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use std::net::SocketAddr;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lightyear::client::input::InputSystemSet;
pub use lightyear::prelude::client::*;
use lightyear::prelude::server::ServerCommandsExt;
use lightyear::prelude::*;

use crate::protocol::*;
use lightyear_examples_common::settings::{get_client_net_config, Settings};

pub struct ExampleClientPlugin {
    pub(crate) settings: Settings,
}

/// State that tracks whether we are in the lobby or in the game
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppState {
    Lobby { joined_lobby: Option<usize> },
    Game,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Lobby { joined_lobby: None }
    }
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.settings.clone());
        app.init_resource::<lobby::LobbyTable>();
        app.init_resource::<Lobbies>();
        app.init_state::<AppState>();
        app.add_systems(Startup, on_disconnect);
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        app.add_systems(
            FixedPreUpdate,
            game::buffer_input
                // Inputs have to be buffered in the FixedPreUpdate schedule
                .in_set(InputSystemSet::WriteClientInputs)
                .run_if(in_state(AppState::Game)),
        );
        app.add_systems(
            FixedUpdate,
            game::player_movement.run_if(in_state(AppState::Game)),
        );
        app.add_systems(
            Update,
            (
                game::handle_predicted_spawn,
                game::handle_interpolated_spawn,
            )
                .run_if(in_state(AppState::Game)),
        );
        app.add_systems(Update, (lobby::lobby_ui, lobby::receive_start_game_message));
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

/// Marker component for the debug text displaying the `ClientId`
#[derive(Component)]
struct ClientIdText;

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
    debug_text: Query<Entity, With<ClientIdText>>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        if let Ok(entity) = debug_text.get_single() {
            commands.entity(entity).despawn();
        }
    }
}

/// Remove all entities when the client disconnect.
/// Reset the ClientConfig to connect to the dedicated server on the next connection attempt.
fn on_disconnect(
    mut commands: Commands,
    entities: Query<Entity, (Without<Window>, Without<Camera2d>)>,
    mut config: ResMut<ClientConfig>,
    settings: Res<Settings>,
    connection: Res<ClientConnection>,
) {
    let existing_client_id = connection.id();

    for entity in entities.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Lobbies>();

    // stop the server if it was started (if the player was host)
    commands.stop_server();

    // update the client config to connect to the lobby server
    config.net = get_client_net_config(settings.as_ref(), existing_client_id.to_bits());
}

mod game {
    use crate::protocol::Direction;
    use crate::shared::shared_movement_behaviour;
    use lightyear::inputs::native::{ActionState, InputMarker};

    use super::*;

    /// System that reads from peripherals and adds inputs to the buffer
    /// This system must be run in the
    pub(crate) fn buffer_input(
        mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
        keypress: Res<ButtonInput<KeyCode>>,
    ) {
        if let Ok(mut action_state) = query.get_single_mut() {
            let mut input = None;
            let mut direction = Direction {
                up: false,
                down: false,
                left: false,
                right: false,
            };
            if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
                direction.up = true;
            }
            if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
                direction.down = true;
            }
            if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
                direction.left = true;
            }
            if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
                direction.right = true;
            }
            if !direction.is_none() {
                input = Some(Inputs::Direction(direction));
            }
            action_state.value = input;
        }
    }

    /// The client input only gets applied to predicted entities that we own
    /// This works because we only predict the user's controlled entity.
    /// If we were predicting more entities, we would have to only apply movement to the player owned one.
    pub(crate) fn player_movement(
        mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
    ) {
        for (position, input) in position_query.iter_mut() {
            if let Some(input) = &input.value {
                // NOTE: be careful to directly pass Mut<PlayerPosition>
                // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
                shared_movement_behaviour(position, input);
            }
        }
    }

    /// When the predicted copy of the client-owned entity is spawned, do stuff
    /// - assign it a different saturation
    /// - add an InputMarker so that we can control the entity
    pub(crate) fn handle_predicted_spawn(
        mut predicted: Query<(Entity, &mut PlayerColor), Added<Predicted>>,
        mut commands: Commands,
    ) {
        for (entity, mut color) in predicted.iter_mut() {
            let hsva = Hsva {
                saturation: 0.4,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            commands
                .entity(entity)
                .insert(InputMarker::<Inputs>::default());
        }
    }

    /// When the predicted copy of the client-owned entity is spawned, do stuff
    /// - assign it a different saturation
    /// - keep track of it in the Global resource
    pub(crate) fn handle_interpolated_spawn(
        mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
    ) {
        for mut color in interpolated.iter_mut() {
            let hsva = Hsva {
                saturation: 0.1,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
        }
    }
}

mod lobby {
    use std::net::SocketAddr;

    use bevy::platform::collections::HashMap;
    use bevy_egui::egui::Separator;
    use bevy_egui::{egui, EguiContexts};
    use egui_extras::{Column, TableBuilder};
    use tracing::{error, info};

    use lightyear::server::config::ServerConfig;

    use crate::client::{lobby, AppState};
    use crate::HOST_SERVER_PORT;

    use super::*;

    #[derive(Resource, Default, Debug)]
    pub(crate) struct LobbyTable {
        clients: HashMap<ClientId, bool>,
    }

    impl LobbyTable {
        /// Find who will be the host of the game. If no client is host; the server will be the host.
        pub(crate) fn get_host(&self) -> Option<ClientId> {
            self.clients
                .iter()
                .find_map(|(client_id, is_host)| if *is_host { Some(*client_id) } else { None })
        }
    }

    /// Display a lobby ui that lets you choose the network topology before starting a game.
    /// Either the game will use a dedicated server as a host, or one of the players will run in host-server mode.
    pub(crate) fn lobby_ui(
        mut commands: Commands,
        mut contexts: EguiContexts,
        mut lobby_table: ResMut<LobbyTable>,
        mut connection_manager: ResMut<ConnectionManager>,
        settings: Res<Settings>,
        config: ResMut<ClientConfig>,
        lobbies: Option<Res<Lobbies>>,
        state: Res<State<NetworkingState>>,
        app_state: Res<State<AppState>>,
        mut next_app_state: ResMut<NextState<AppState>>,
    ) {
        let window_name = match app_state.get() {
            AppState::Lobby { joined_lobby } => {
                joined_lobby.map_or("Lobby List".to_string(), |i| format!("Lobby {i}"))
            }
            AppState::Game => "Game".to_string(),
        };
        egui::Window::new(window_name)
            .anchor(egui::Align2::LEFT_TOP, [30.0, 30.0])
            .show(contexts.ctx_mut(), |ui| {
                match app_state.get() {
                    AppState::Lobby { joined_lobby } => {
                        if joined_lobby.is_none() {
                            let table = TableBuilder::new(ui)
                                .resizable(false)
                                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                .column(Column::auto())
                                .column(Column::auto())
                                .column(Column::auto())
                                .column(Column::auto());
                            table
                                .header(20.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Lobby ID");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Number of players");
                                    });
                                    header.col(|ui| {
                                        ui.strong("In Game?");
                                    });
                                    header.col(|ui| {
                                        ui.strong("");
                                    });
                                })
                                .body(|mut body| {
                                    body.row(30.0, |mut row| {
                                        row.col(|ui| {
                                            ui.label("Server");
                                        });
                                        row.col(|ui| {});
                                    });
                                    if let Some(lobbies) = lobbies {
                                        for (lobby_id, lobby) in lobbies.lobbies.iter().enumerate()
                                        {
                                            body.row(30.0, |mut row| {
                                                row.col(|ui| {
                                                    ui.label(format!("Lobby {lobby_id:?}"));
                                                });
                                                row.col(|ui| {
                                                    ui.label(format!("{}", lobby.players.len()));
                                                });
                                                row.col(|ui| {
                                                    ui.checkbox(&mut { lobby.in_game }, "");
                                                });
                                                row.col(|ui| {
                                                    if lobby.in_game {
                                                        if ui.button("Join Game").clicked() {
                                                            // find the host of the game
                                                            let host = lobby_table.get_host();
                                                            // send a message to join the game
                                                            let _ = connection_manager
                                                                .send_message::<Channel1, _>(
                                                                    &mut StartGame {
                                                                        lobby_id,
                                                                        host,
                                                                    },
                                                                );
                                                        }
                                                    } else {
                                                        if ui.button("Join Lobby").clicked() {
                                                            connection_manager
                                                                .send_message::<Channel1, _>(
                                                                    &mut JoinLobby { lobby_id },
                                                                )
                                                                .unwrap();
                                                            next_app_state.set(AppState::Lobby {
                                                                joined_lobby: Some(lobby_id),
                                                            });
                                                        }
                                                    }
                                                });
                                            });
                                        }
                                    }
                                });
                        } else {
                            let joined_lobby = joined_lobby.unwrap();
                            let lobby =
                                lobbies.as_ref().unwrap().lobbies.get(joined_lobby).unwrap();
                            let table = TableBuilder::new(ui)
                                .resizable(false)
                                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                .column(Column::auto())
                                .column(Column::auto());
                            table
                                .header(20.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Client ID");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Host");
                                    });
                                })
                                .body(|mut body| {
                                    body.row(30.0, |mut row| {
                                        row.col(|ui| {
                                            ui.label("Server");
                                        });
                                        row.col(|ui| {});
                                    });
                                    for client_id in &lobby.players {
                                        lobby_table.clients.entry(*client_id).or_insert(false);
                                        body.row(30.0, |mut row| {
                                            row.col(|ui| {
                                                ui.label(format!("{client_id:?}"));
                                            });
                                            row.col(|ui| {
                                                ui.checkbox(
                                                    lobby_table.clients.get_mut(client_id).unwrap(),
                                                    "",
                                                );
                                            });
                                        });
                                    }
                                });
                        }
                        ui.add(Separator::default().horizontal());
                    }
                    AppState::Game => {}
                };
                match state.get() {
                    NetworkingState::Disconnected | NetworkingState::Disconnecting => {
                        if ui.button("Join lobby list").clicked() {
                            // TODO: before connecting, we want to adjust all clients ConnectionConfig to respect the new host
                            // - the new host must run in host-server
                            // - all clients must adjust their net-config to connect to the host
                            commands.connect_client();
                        }
                    }
                    NetworkingState::Connecting => {
                        let _ = ui.button("Connecting");
                    }
                    NetworkingState::Connected => {
                        match app_state.get() {
                            AppState::Lobby { joined_lobby } => {
                                if let Some(lobby_id) = joined_lobby {
                                    if ui.button("Exit lobby").clicked() {
                                        connection_manager
                                            .send_message::<Channel1, _>(&mut ExitLobby {
                                                lobby_id: *lobby_id,
                                            })
                                            .unwrap();
                                        next_app_state.set(AppState::Lobby { joined_lobby: None });
                                    }
                                    if ui.button("Start game").clicked() {
                                        // find the host of the game
                                        let host = lobby_table.get_host();
                                        // send a message to server/client to start the game and possibly act as server
                                        let _ = connection_manager.send_message::<Channel1, _>(
                                            &mut StartGame {
                                                lobby_id: *lobby_id,
                                                host,
                                            },
                                        );
                                    }
                                } else {
                                    if ui.button("Exit lobby list").clicked() {
                                        commands.disconnect_client();
                                    }
                                }
                            }
                            AppState::Game => {
                                if ui.button("Exit game").clicked() {
                                    next_app_state.set(AppState::Lobby { joined_lobby: None });
                                    commands.disconnect_client();
                                }
                            }
                        }
                    }
                }
            });
    }

    /// Listen for the StartGame message, and start the game if it was (which means that a client clicked on the 'start game' button)
    /// - update the client config to connect to the game host (either the server or one of the other clients)
    /// - connect by setting the NetworkingState to Connecting
    /// - set the AppState to Game
    pub(crate) fn receive_start_game_message(
        mut commands: Commands,
        mut events: EventReader<ReceiveMessage<StartGame>>,
        lobby_table: Res<LobbyTable>,
        mut next_app_state: ResMut<NextState<AppState>>,
        mut config: ResMut<ClientConfig>,
        settings: Res<Settings>,
        connection: Res<ClientConnection>,
    ) {
        for event in events.read() {
            let host = event.message().host;
            let lobby_id = event.message().lobby_id;
            // set the state to Game
            next_app_state.set(AppState::Game);
            // the host of the game is another player
            if let Some(host) = host {
                if host == connection.id() {
                    info!("We are the host of the game!");
                    // set the client connection to be local
                    config.net = NetConfig::Local { id: host.to_bits() };
                    // start the server
                    commands.start_server();
                } else {
                    info!("The game is hosted by another client. Connecting to the host...");
                    // update the client config to connect to the game host
                    match &mut config.net {
                        NetConfig::Netcode { auth, config, io } => match auth {
                            Authentication::Manual { server_addr, .. } => {
                                *server_addr = SocketAddr::new(
                                    settings.client.server_addr.into(),
                                    HOST_SERVER_PORT,
                                );
                            }
                            _ => {}
                        },
                        _ => {
                            error!("Unsupported net config");
                        }
                    }
                }
                // start the connection process
                commands.connect_client();
            }
        }
    }
}
