use core::net::{Ipv4Addr, SocketAddr};

use crate::protocol::*;
use crate::HOST_SERVER_PORT;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lightyear::input::client::InputSet;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::server::Stop;
use lightyear::prelude::*;
use lightyear_examples_common::shared::{SERVER_PORT, SHARED_SETTINGS};

pub struct ExampleClientPlugin;

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
        app.init_resource::<lobby::LobbyTable>();
        app.init_state::<AppState>();
        app.add_systems(
            FixedPreUpdate,
            game::buffer_input
                .in_set(InputSet::WriteClientInputs)
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
        app.add_systems(
            PostUpdate,
            (lobby::lobby_ui, lobby::receive_start_game_message),
        );
        app.add_observer(on_disconnect);
    }
}

/// Remove all entities when the client disconnect.
/// Reset the ClientConfig to connect to the dedicated server on the next connection attempt.
fn on_disconnect(
    trigger: Trigger<OnAdd, Disconnected>,
    local_id: Single<&LocalId>,
    lobbies: Query<Entity, With<Lobbies>>,
    mut commands: Commands,
    entities: Query<Entity, (With<Lobbies>, With<PlayerId>)>,
) -> Result {
    // despawn every entity
    for entity in entities.iter() {
        commands.entity(entity).despawn();
    }

    // stop the server if it was started (if the player was host)
    commands.trigger_targets(Stop, trigger.target());

    // reset the netcode config to connect to the lobby server
    let host_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT);
    let auth = Authentication::Manual {
        server_addr: host_addr,
        client_id: local_id.0.to_bits(),
        private_key: SHARED_SETTINGS.private_key,
        protocol_id: SHARED_SETTINGS.protocol_id,
    };
    let netcode_config = NetcodeConfig {
        // Make sure that the server times out clients when their connection is closed
        client_timeout_secs: 3,
        token_expire_secs: -1,
        ..default()
    };
    commands
        .entity(trigger.target())
        .insert(NetcodeClient::new(auth, netcode_config)?);
    Ok(())
}

mod game {
    use crate::protocol::Direction;
    use crate::shared::shared_movement_behaviour;
    use lightyear::input::native::prelude::{ActionState, InputMarker};

    use super::*;

    /// System that reads from peripherals and adds inputs to the buffer
    /// This system must be run in the
    pub(crate) fn buffer_input(
        mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
        keypress: Res<ButtonInput<KeyCode>>,
    ) {
        if let Ok(mut action_state) = query.single_mut() {
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
            action_state.value = Some(Inputs::Direction(direction));
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
    use core::net::{Ipv4Addr, SocketAddr};

    use super::*;
    use crate::client::{lobby, AppState};
    use crate::HOST_SERVER_PORT;
    use bevy::platform::collections::HashMap;
    use bevy_egui::egui::Separator;
    use bevy_egui::{egui, EguiContexts};
    use egui_extras::{Column, TableBuilder};
    use lightyear::connection::client::ClientState;
    use lightyear::connection::server::Start;
    use lightyear::netcode::client_plugin::NetcodeConfig;
    use lightyear::netcode::NetcodeClient;
    use lightyear::prelude::PeerId::Netcode;
    use lightyear_examples_common::shared::SHARED_SETTINGS;
    use tracing::{error, info};

    #[derive(Resource, Default, Debug)]
    pub(crate) struct LobbyTable {
        clients: HashMap<PeerId, bool>,
    }

    impl LobbyTable {
        /// Find who will be the host of the game. If no client is host; the server will be the host.
        pub(crate) fn get_host(&self) -> Option<PeerId> {
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
        lobbies: Option<Single<&Lobbies>>,
        message_sender: Single<(
            Entity,
            &Client,
            &mut MessageSender<StartGame>,
            &mut MessageSender<JoinLobby>,
            &mut MessageSender<ExitLobby>,
        )>,
        app_state: Res<State<AppState>>,
        mut next_app_state: ResMut<NextState<AppState>>,
    ) {
        let (client_entity, client, mut send_start_game, mut send_join_lobby, mut exit_lobby) =
            message_sender.into_inner();
        let window_name = match app_state.get() {
            AppState::Lobby { joined_lobby } => {
                joined_lobby.map_or("Lobby List".to_string(), |i| format!("Lobby {i}"))
            }
            AppState::Game => "Game".to_string(),
        };
        egui::Window::new(window_name)
            .anchor(egui::Align2::LEFT_TOP, [30.0, 30.0])
            .show(contexts.ctx_mut().unwrap(), |ui| {
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
                                                            info!("Lobby {lobby_id} starting game with host {host:?}");
                                                            // send a message to join the game
                                                            send_start_game.send::<Channel1>(StartGame {
                                                                lobby_id,
                                                                host,
                                                            });
                                                        }
                                                    } else if ui.button("Join Lobby").clicked() {
                                                        info!("Client joining lobby {lobby_id}");
                                                        send_join_lobby.send::<Channel1>(JoinLobby { lobby_id });
                                                        next_app_state.set(AppState::Lobby {
                                                            joined_lobby: Some(lobby_id),
                                                        });
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
                match client.state {
                    ClientState::Disconnected | ClientState::Disconnecting => {
                        if ui.button("Join lobby list").clicked() {
                            commands.trigger_targets(Connect, client_entity);
                        }
                    }
                    ClientState::Connecting => {
                        let _ = ui.button("Connecting");
                    }
                    ClientState::Connected => {
                        match app_state.get() {
                            AppState::Lobby { joined_lobby } => {
                                if let Some(lobby_id) = joined_lobby {
                                    if ui.button("Exit lobby").clicked() {
                                        info!("Exit lobby {lobby_id:?}");
                                        exit_lobby.send::<Channel1>(ExitLobby {
                                            lobby_id: *lobby_id
                                        });
                                        next_app_state.set(AppState::Lobby { joined_lobby: None });
                                    }
                                    if ui.button("Start game").clicked() {
                                        // find the host of the game
                                        let host = lobby_table.get_host();
                                        info!("Starting game for lobby {lobby_id:?}! Host is {host:?}");
                                        // send a message to server/client to start the game and possibly act as server
                                        send_start_game.send::<Channel1>(StartGame {
                                            lobby_id: *lobby_id,
                                            host,
                                        });
                                    }
                                } else if ui.button("Exit lobby list").clicked() {
                                    commands.trigger_targets(Disconnect, client_entity);
                                }
                            }
                            AppState::Game => {
                                if ui.button("Exit game").clicked() {
                                    next_app_state.set(AppState::Lobby { joined_lobby: None });
                                    commands.trigger_targets(Disconnect, client_entity);
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
        local_client: Single<(Entity, &mut MessageReceiver<StartGame>, &LocalId)>,
        lobby_table: Res<LobbyTable>,
        mut next_app_state: ResMut<NextState<AppState>>,
        server: Single<Entity, With<Server>>,
    ) -> Result {
        let server = server.into_inner();
        let (local_client, mut receiver, local_id) = local_client.into_inner();
        for message in receiver.receive() {
            info!("Received start_game message! {message:?}");
            let host = message.host;
            // set the state to Game
            next_app_state.set(AppState::Game);
            // the host of the game is another player
            if let Some(host) = host {
                if host == local_id.0 {
                    info!("We are the host of the game!");
                    // First clone the previous link
                    commands.trigger_targets(
                        Unlink {
                            reason: "Client becoming Host".to_string(),
                        },
                        local_client,
                    );

                    // Remove any previous networking components
                    commands.entity(local_client).remove::<NetcodeClient>();

                    // Any entity that is both a Client and a LinkOf will be a host-client.
                    // The corresponding server will be a HostServer.
                    commands.entity(local_client).insert(LinkOf { server });
                    info!("Connecting as a Host Client");
                } else {
                    info!(
                        "The game is hosted by another client ({host:?}). Connecting to the host..."
                    );
                    // First unlink from the dedicated server
                    commands.trigger_targets(
                        Unlink {
                            reason: "Connecting to host-client".to_string(),
                        },
                        local_client,
                    );

                    let host_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), HOST_SERVER_PORT);
                    let auth = Authentication::Manual {
                        server_addr: host_addr,
                        client_id: local_id.0.to_bits(),
                        private_key: SHARED_SETTINGS.private_key,
                        protocol_id: SHARED_SETTINGS.protocol_id,
                    };
                    let netcode_config = NetcodeConfig {
                        // Make sure that the server times out clients when their connection is closed
                        client_timeout_secs: 3,
                        token_expire_secs: -1,
                        ..default()
                    };
                    let netcode_client = commands.entity(local_client).insert((
                        NetcodeClient::new(auth, netcode_config)?,
                        PeerAddr(host_addr),
                    ));
                }
                // Trigger a `Connection` to update the connection settings.
                commands.trigger_targets(Connect, local_client);
            }
        }
        Ok(())
    }
}
