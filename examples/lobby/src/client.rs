//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;
use bevy_egui::{egui, EguiContexts};
use bevy_mod_picking::picking_core::Pickable;
use bevy_mod_picking::prelude::{Click, On, Pointer};

pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, ClientTransports, SharedSettings};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ui::LobbyTable>();
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        // Inputs have to be buffered in the FixedPreUpdate schedule
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (
                receive_client_connection,
                receive_client_disconnection,
                receive_entity_spawn,
                receive_entity_despawn,
                handle_predicted_spawn,
                handle_interpolated_spawn,
                ui::lobby_ui,
            ),
        );
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

/// Component to identify the text displaying the client id
#[derive(Component)]
pub struct ClientIdText;

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
pub(crate) fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
    mut lobby_table: ResMut<ui::LobbyTable>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        lobby_table.clients.insert(client_id, false);
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
fn player_movement(
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

/// System handling receiving a ClientConnection message
pub(crate) fn receive_client_connection(
    mut reader: EventReader<MessageEvent<ClientConnect>>,
    mut lobby_table: ResMut<ui::LobbyTable>,
) {
    for event in reader.read() {
        let client_connection = event.message();
        lobby_table.clients.insert(client_connection.id, false);
    }
}

/// System handling receiving a ClientDisconnection message
pub(crate) fn receive_client_disconnection(
    mut reader: EventReader<MessageEvent<ClientDisconnect>>,
    mut lobby_table: ResMut<ui::LobbyTable>,
) {
    for event in reader.read() {
        let client_disconnection = event.message();
        lobby_table.clients.remove(&client_disconnection.id);
    }
}

/// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_spawn(mut reader: EventReader<EntitySpawnEvent>) {
    for event in reader.read() {
        info!("Received entity spawn: {:?}", event.entity());
    }
}

/// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_despawn(mut reader: EventReader<EntityDespawnEvent>) {
    for event in reader.read() {
        info!("Received entity despawn: {:?}", event.entity());
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

/// Remove all entities when the client disconnect
fn on_disconnect(
    mut commands: Commands,
    player_entities: Query<Entity, With<PlayerId>>,
    debug_text: Query<Entity, With<ClientIdText>>,
) {
    for entity in player_entities.iter() {
        commands.entity(entity).despawn_recursive();
    }
    for entity in debug_text.iter() {
        commands.entity(entity).despawn_recursive();
    }
}

mod ui {
    use crate::client::ui;
    use bevy::prelude::{Res, ResMut, Resource, State};
    use bevy::utils::HashMap;
    use bevy_egui::{egui, EguiContexts};
    use egui_extras::{Column, TableBuilder};
    use lightyear::prelude::client::{ClientConnectionParam, NetworkingState};
    use lightyear::prelude::ClientId;
    use tracing::error;

    #[derive(Resource, Default, Debug)]
    pub(crate) struct LobbyTable {
        /// map from the client_id to a boolean indicating if the client is the host
        pub(crate) clients: HashMap<ClientId, bool>,
        /// true if we will use the server as host
        pub server: bool,
    }

    impl LobbyTable {
        fn table_ui(
            &mut self,
            ui: &mut egui::Ui,
            state: &NetworkingState,
            connection: &mut ClientConnectionParam,
        ) {
            let table = TableBuilder::new(ui)
                .resizable(true)
                .sense(egui::Sense::click())
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
                        row.col(|ui| {
                            ui.toggle_value(&mut self.server, "");
                        });
                    });
                    for (client_id, is_host) in self.clients.iter_mut() {
                        body.row(30.0, |mut row| {
                            row.col(|ui| {
                                ui.label(format!("{client_id:?}"));
                            });
                            row.col(|ui| {
                                ui.toggle_value(is_host, "");
                            });
                        });
                    }
                });
            ui.separator();
            match state {
                NetworkingState::Disconnected => {
                    if ui.button("Connect").clicked() {
                        // TODO: before connecting, we want to adjust all clients ConnectionConfig to respect the new host
                        // - the new host must run in host-server
                        // - all clients must adjust their net-config to connect to the host
                        let _ = connection
                            .connect()
                            .inspect_err(|e| error!("Failed to connect: {e:?}"));
                    }
                }
                NetworkingState::Connecting => {
                    let _ = ui.button("Connecting");
                }
                NetworkingState::Connected => {
                    // TODO: should the client be able to connect to multiple servers?
                    //  (for example so that it's connected to the lobby-server at the same time
                    //  as the game-server)
                    // TODO: disconnect from the current game, adjust the client-config, and join the dedicated server
                    if ui.button("Disconnect").clicked() {
                        let _ = connection
                            .disconnect()
                            .inspect_err(|e| error!("Failed to disconnect: {e:?}"));
                    }
                }
            }
        }
    }

    /// Display a lobby ui that lets you choose the network topology before starting a game.
    /// Either the game will use a dedicated server as a host, or one of the players will run in host-server mode.
    pub(crate) fn lobby_ui(
        mut contexts: EguiContexts,
        mut lobby_table: ResMut<LobbyTable>,
        state: Res<State<NetworkingState>>,
        mut connection: ClientConnectionParam,
    ) {
        egui::Window::new("Lobby").show(contexts.ctx_mut(), |ui| {
            lobby_table.table_ui(ui, state.get(), &mut connection);
        });
    }
}
