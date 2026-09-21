use core::net::{Ipv4Addr, SocketAddr};

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use lightyear::input::client::InputSystems;
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

#[derive(Component)]
struct SwitchingGameConnection;

/// Application identity survives replacement of a network connection.
#[derive(Component, Clone, Copy)]
pub(crate) struct LobbyClientId(pub u64);

fn spawn_remote_client(commands: &mut Commands, id: LobbyClientId, address: SocketAddr) -> Entity {
    use lightyear_examples_common::client::{ClientTransports, ExampleClient};
    commands
        .spawn((
            id,
            ExampleClient {
                client_id: id.0,
                client_port: 0,
                server_addr: address,
                conditioner: Some(RecvLinkConditioner::new(
                    LinkConditionerConfig::average_condition().half(),
                )),
                transport: ClientTransports::WebTransport,
                shared: SHARED_SETTINGS,
            },
        ))
        .id()
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Lobby { joined_lobby: None }
    }
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.init_resource::<lobby::LobbyTable>();
        app.init_state::<AppState>();
        app.add_systems(
            FixedPreUpdate,
            game::buffer_input
                .in_set(InputSystems::WriteClientInputs)
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
                game::handle_controlled_spawn,
                game::handle_interpolated_spawn,
            )
                .run_if(in_state(AppState::Game)),
        );
        #[cfg(feature = "gui")]
        app.add_systems(EguiPrimaryContextPass, lobby::lobby_ui);
        app.add_systems(
            PreUpdate,
            lobby::receive_start_game_message.after(MessageSystems::Receive),
        );
        app.add_observer(on_disconnect);
    }
}

/// Return to a fresh lobby connection after disconnecting from a game.
fn on_disconnect(
    trigger: On<Add, Disconnected>,
    clients: Query<(&LobbyClientId, &LocalId), With<Client>>,
    server: Single<Entity, With<Server>>,
    mut commands: Commands,
    switching_game_connection: Query<(), With<SwitchingGameConnection>>,
    mut next_app_state: ResMut<NextState<AppState>>,
) {
    // Newly constructed backends start Disconnected, before they have a resolved LocalId.
    let Ok((id, _)) = clients.get(trigger.entity) else {
        return;
    };
    if switching_game_connection.contains(trigger.entity) {
        return;
    }
    // Replication and server ownership perform entity cleanup during teardown.
    commands.trigger(Stop { entity: *server });
    commands.entity(trigger.entity).despawn();
    spawn_remote_client(
        &mut commands,
        *id,
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT),
    );
    next_app_state.set(AppState::Lobby { joined_lobby: None });
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
            action_state.0 = Inputs::Direction(direction);
        }
    }

    /// Applies local movement only to predicted entities owned by this client.
    ///
    /// If this example predicted remote entities, ownership would need to be checked before movement.
    pub(crate) fn player_movement(
        mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
    ) {
        for (position, input) in position_query.iter_mut() {
            // Pass Mut<PlayerPosition> directly so change detection only fires when movement changes it.
            shared_movement_behaviour(position, input);
        }
    }

    /// Lower the saturation on predicted entities so they are visually distinct.
    pub(crate) fn handle_predicted_spawn(
        mut predicted: Query<(Entity, &mut PlayerColor), Added<Predicted>>,
    ) {
        for (entity, mut color) in predicted.iter_mut() {
            let hsva = Hsva {
                saturation: 0.4,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
        }
    }

    /// Add the local input marker once ownership is known.
    pub(crate) fn handle_controlled_spawn(
        controlled: Query<
            Entity,
            (
                With<Controlled>,
                With<PlayerId>,
                Without<InputMarker<Inputs>>,
            ),
        >,
        mut commands: Commands,
    ) {
        for entity in controlled.iter() {
            commands
                .entity(entity)
                .insert(InputMarker::<Inputs>::default());
        }
    }

    /// Lower the saturation on interpolated entities so they are visually distinct.
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
    use crate::host_server_port;
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
        message_sender: Single<
            (
                Entity,
                ClientState,
                &mut MessageSender<StartGame>,
                &mut MessageSender<JoinLobby>,
                &mut MessageSender<ExitLobby>,
            ),
            With<Client>,
        >,
        app_state: Res<State<AppState>>,
        mut next_app_state: ResMut<NextState<AppState>>,
    ) -> Result {
        let (client_entity, state, mut send_start_game, mut send_join_lobby, mut exit_lobby) =
            message_sender.into_inner();
        let window_name = match app_state.get() {
            AppState::Lobby { joined_lobby } => {
                joined_lobby.map_or("Lobby List".to_string(), |i| format!("Lobby {i}"))
            }
            AppState::Game => "Game".to_string(),
        };
        egui::Window::new(window_name)
            .anchor(egui::Align2::LEFT_TOP, [30.0, 30.0])
            .show(contexts.ctx_mut()?, |ui| {
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
                if state.is_disconnected() || state.is_disconnecting() {
                        if ui.button("Join lobby list").clicked() {
                            commands.trigger(Connect { entity: client_entity });
                        }
                } else if state.is_connecting() {
                        let _ = ui.button("Connecting");
                } else if state.is_connected() {
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
                                    commands.trigger(Disconnect { entity: client_entity });
                                }
                            }
                            AppState::Game => {
                                if ui.button("Exit game").clicked() {
                                    next_app_state.set(AppState::Lobby { joined_lobby: None });
                                    commands.trigger(Disconnect {entity: client_entity });
                                }
                            }
                        }
                }
            });
        Ok(())
    }

    /// Replace the lobby connection with a connection to the selected game host.
    pub(crate) fn receive_start_game_message(
        mut commands: Commands,
        local_client: Single<(
            Entity,
            &mut MessageReceiver<StartGame>,
            &LocalId,
            &LobbyClientId,
        )>,
        mut next_app_state: ResMut<NextState<AppState>>,
        server: Single<Entity, With<Server>>,
    ) {
        let server = server.into_inner();
        let (local_client, mut receiver, local_id, account_id) = local_client.into_inner();
        let Some(message) = receiver.receive().next() else {
            return;
        };
        info!("Received start_game message! {message:?}");
        next_app_state.set(AppState::Game);
        let Some(host) = message.host else {
            return;
        };

        commands
            .entity(local_client)
            .insert(SwitchingGameConnection);
        commands.trigger(Disconnect {
            entity: local_client,
        });
        commands.entity(local_client).despawn();

        // Backend ownership is fixed for an entity, including after disconnection.
        let client = if host == local_id.0 {
            commands.trigger(Start { entity: server });
            commands
                .spawn((
                    *account_id,
                    Client,
                    ReplicationReceiver,
                    LinkOf { endpoint: server },
                ))
                .id()
        } else {
            spawn_remote_client(
                &mut commands,
                *account_id,
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), host_server_port(host)),
            )
        };
        commands.trigger(Connect { entity: client });
    }
}
