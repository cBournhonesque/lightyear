//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use crate::protocol::*;
use crate::settings::Settings;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::SocketAddr;

pub struct ExampleClientPlugin {
    pub(crate) settings: Settings,
}

/// State that tracks whether we are in the lobby or in the game
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    Lobby,
    Game,
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.settings.clone());
        app.init_resource::<lobby::LobbyTable>();
        app.init_resource::<Lobby>();
        app.init_state::<AppState>();
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        app.add_systems(
            FixedPreUpdate,
            game::buffer_input
                // Inputs have to be buffered in the FixedPreUpdate schedule
                .in_set(InputSystemSet::BufferInputs)
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
                game::exit_game_button,
            )
                .run_if(in_state(AppState::Game)),
        );
        app.add_systems(
            Update,
            (lobby::lobby_ui, lobby::receive_start_game_message).run_if(in_state(AppState::Lobby)),
        );
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
    debug_text: Query<(), With<ClientIdText>>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        if debug_text.is_empty() {
            commands.spawn((
                TextBundle::from_section(
                    format!("Client {}", client_id),
                    TextStyle {
                        font_size: 30.0,
                        color: Color::WHITE,
                        ..default()
                    },
                ),
                ClientIdText,
            ));
        }
    }
}

/// Remove all entities when the client disconnect
fn on_disconnect(
    mut commands: Commands,
    entities: Query<Entity, (Without<Window>, Without<Camera2d>)>,
    mut config: ResMut<ClientConfig>,
    settings: Res<Settings>,
) {
    for entity in entities.iter() {
        commands.entity(entity).despawn_recursive();
    }

    // update the client config to connect to the lobby server
    match &mut config.net {
        NetConfig::Netcode { auth, .. } => match auth {
            Authentication::Manual { server_addr, .. } => {
                *server_addr = SocketAddr::new(
                    settings.client.server_addr.into(),
                    settings.client.lobby_server_port.into(),
                );
            }
            _ => {}
        },
        _ => {
            error!("Unsupported net config");
        }
    }
}

mod game {
    use super::*;
    use crate::protocol::Direction;
    use crate::shared::shared_movement_behaviour;

    /// System that reads from peripherals and adds inputs to the buffer
    /// This system must be run in the
    pub(crate) fn buffer_input(
        tick_manager: Res<TickManager>,
        mut input_manager: ResMut<InputManager<Inputs>>,
        keypress: Res<ButtonInput<KeyCode>>,
    ) {
        let tick = tick_manager.tick();
        let mut input = Inputs::None;
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
            input = Inputs::Direction(direction);
        }
        if keypress.pressed(KeyCode::Backspace) {
            input = Inputs::Delete;
        }
        if keypress.pressed(KeyCode::Space) {
            input = Inputs::Spawn;
        }
        input_manager.add_input(input, tick)
    }

    /// The client input only gets applied to predicted entities that we own
    /// This works because we only predict the user's controlled entity.
    /// If we were predicting more entities, we would have to only apply movement to the player owned one.
    pub(crate) fn player_movement(
        // TODO: maybe make prediction mode a separate component!!!
        mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
        mut input_reader: EventReader<InputEvent<Inputs>>,
    ) {
        if <Components as SyncMetadata<PlayerPosition>>::mode() != ComponentSyncMode::Full {
            return;
        }
        for input in input_reader.read() {
            if let Some(input) = input.input() {
                for position in position_query.iter_mut() {
                    shared_movement_behaviour(position, input);
                }
            }
        }
    }

    /// When the predicted copy of the client-owned entity is spawned, do stuff
    /// - assign it a different saturation
    /// - keep track of it in the Global resource
    pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
        for mut color in predicted.iter_mut() {
            color.0.set_s(0.3);
        }
    }

    /// When the predicted copy of the client-owned entity is spawned, do stuff
    /// - assign it a different saturation
    /// - keep track of it in the Global resource
    pub(crate) fn handle_interpolated_spawn(
        mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
    ) {
        for mut color in interpolated.iter_mut() {
            color.0.set_s(0.1);
        }
    }

    /// Button shown during the game; when clicked, the client exits the game and rejoins the lobby ui
    pub(crate) fn exit_game_button(
        mut contexts: EguiContexts,
        mut next_app_state: ResMut<NextState<AppState>>,
        mut next_state: ResMut<NextState<NetworkingState>>,
    ) {
        egui::Window::new("Lobby").show(contexts.ctx_mut(), |ui| {
            if ui.button("Exit game").clicked() {
                next_app_state.set(AppState::Lobby);
                next_state.set(NetworkingState::Disconnected);
            }
        });
    }
}

mod lobby {
    use super::*;
    use crate::client::{lobby, AppState};
    use bevy::utils::HashMap;
    use bevy_egui::egui::Separator;
    use bevy_egui::{egui, EguiContexts};
    use egui_extras::{Column, TableBuilder};
    use std::net::SocketAddr;
    use tracing::{error, info};

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
        mut contexts: EguiContexts,
        mut lobby_table: ResMut<LobbyTable>,
        mut connection_manager: ResMut<ClientConnectionManager>,
        settings: Res<Settings>,
        mut config: ResMut<ClientConfig>,
        lobby: Option<Res<Lobby>>,
        state: Res<State<NetworkingState>>,
        mut next_state: ResMut<NextState<NetworkingState>>,
        mut next_app_state: ResMut<NextState<AppState>>,
    ) {
        egui::Window::new("Lobby").show(contexts.ctx_mut(), |ui| {
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
                    if let Some(lobby) = lobby {
                        for client_id in lobby.players.iter() {
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
                    }
                });
            ui.add(Separator::default().horizontal());

            match state.get() {
                NetworkingState::Disconnected => {
                    if ui.button("Join lobby").clicked() {
                        // TODO: before connecting, we want to adjust all clients ConnectionConfig to respect the new host
                        // - the new host must run in host-server
                        // - all clients must adjust their net-config to connect to the host
                        next_state.set(NetworkingState::Connecting);
                    }
                }
                NetworkingState::Connecting => {
                    let _ = ui.button("Joining lobby");
                }
                NetworkingState::Connected => {
                    // TODO: should the client be able to connect to multiple servers?
                    //  (for example so that it's connected to the lobby-server at the same time
                    //  as the game-server)
                    // TODO: disconnect from the current game, adjust the client-config, and join the dedicated server
                    if ui.button("Exit lobby").clicked() {
                        // disconnect from the lobby
                        next_state.set(NetworkingState::Disconnected);
                    }
                    if ui.button("Start game").clicked() {
                        // find the host of the game
                        let host = lobby_table.get_host();
                        // update the client config to connect to the game server
                        match &mut config.net {
                            NetConfig::Netcode { auth, .. } => match auth {
                                Authentication::Manual { server_addr, .. } => {
                                    *server_addr = SocketAddr::new(
                                        settings.client.server_addr.into(),
                                        settings.client.server_port,
                                    );
                                }
                                _ => {}
                            },
                            _ => {
                                error!("Unsupported net config");
                            }
                        }
                        // set the state to Game
                        next_app_state.set(AppState::Game);
                        // send a message to server/client to start the game and act as server
                        let _ = connection_manager.send_message_to_target::<Channel1, _>(
                            StartGame { host },
                            NetworkTarget::All,
                        );
                        // start the connection process
                        next_state.set(NetworkingState::Connecting);
                    }
                }
            }
        });
    }

    /// Listen for the StartGame message (which means that a client clicked on the 'start game' button)
    /// - update the client config to connect to the game host (either the server or one of the other clients)
    /// - connect by setting the NetworkingState to Connecting
    /// - set the AppState to Game
    pub(crate) fn receive_start_game_message(
        mut events: EventReader<MessageEvent<StartGame>>,
        lobby_table: Res<LobbyTable>,
        mut next_app_state: ResMut<NextState<AppState>>,
        mut next_state: ResMut<NextState<NetworkingState>>,
        mut config: ResMut<ClientConfig>,
        settings: Res<Settings>,
    ) {
        for event in events.read() {
            // TODO: maybe we can get the host from the lobby table itself?
            // let host = event.message().host;
            // // find the host of the game
            // let host = lobby_table.get_host();
            // update the client config to connect to the game server
            match &mut config.net {
                NetConfig::Netcode { auth, .. } => match auth {
                    Authentication::Manual { server_addr, .. } => {
                        *server_addr = SocketAddr::new(
                            settings.client.server_addr.into(),
                            settings.client.server_port,
                        );
                    }
                    _ => {}
                },
                _ => {
                    error!("Unsupported net config");
                }
            }
            // set the state to Game
            next_app_state.set(AppState::Game);

            // start the connection process
            next_state.set(NetworkingState::Connecting);
        }
    }
}
