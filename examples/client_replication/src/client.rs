use bevy::prelude::*;
use bevy::utils::Duration;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{color_from_id, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        // Inputs need to be buffered in the `FixedPreUpdate` schedule
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        // all actions related-system that can be rolled back should be in the `FixedUpdate` schedule
        app.add_systems(FixedUpdate, (player_movement, delete_player));
        app.add_systems(
            Update,
            (
                cursor_movement,
                receive_message,
                send_message,
                spawn_player,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

/// Startup system for the client
pub(crate) fn init(mut commands: Commands) {
    commands.connect_client();
}

/// Listen for events to know when the client is connected;
/// - spawn a text entity to display the client id
/// - spawn a client-owned cursor entity that will be replicated to the server
pub(crate) fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        commands.spawn(TextBundle::from_section(
            format!("Client {}", client_id),
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        ));
        info!("Spawning local cursor");
        // spawn a local cursor which will be replicated to other clients, but remain client-authoritative.
        commands.spawn(CursorBundle::new(
            client_id,
            Vec2::ZERO,
            color_from_id(client_id),
        ));
    }
}

// System that reads from peripherals and adds inputs to the buffer
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
    if keypress.pressed(KeyCode::KeyK) {
        input = Inputs::Delete;
    }
    if keypress.pressed(KeyCode::Space) {
        input = Inputs::Spawn;
    }
    input_manager.add_input(input, tick);
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    // InputEvent is a special case: we get an event for every fixed-update system run instead of every frame!
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for position in position_query.iter_mut() {
                // NOTE: be careful to directly pass Mut<PlayerPosition>
                // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
                shared_movement_behaviour(position, input);
            }
        }
    }
}

/// Spawn a server-owned pre-predicted player entity when the space command is pressed
fn spawn_player(
    mut commands: Commands,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    connection: Res<ClientConnection>,
    players: Query<&PlayerId, With<PlayerPosition>>,
) {
    let client_id = connection.id();

    // do not spawn a new player if we already have one
    for player_id in players.iter() {
        if player_id.0 == client_id {
            return;
        }
    }
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            match input {
                Inputs::Spawn => {
                    debug!("got spawn input");
                    commands.spawn((
                        PlayerBundle::new(client_id, Vec2::ZERO),
                        // IMPORTANT: this lets the server know that the entity is pre-predicted
                        // when the server replicates this entity; we will get a Confirmed entity which will use this entity
                        // as the Predicted version
                        PrePredicted::default(),
                    ));
                }
                _ => {}
            }
        }
    }
}

/// Delete the predicted player when the space command is pressed
fn delete_player(
    mut commands: Commands,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    connection: Res<ClientConnection>,
    players: Query<
        (Entity, &PlayerId),
        (
            With<PlayerPosition>,
            Without<Confirmed>,
            Without<Interpolated>,
        ),
    >,
) {
    let client_id = connection.id();
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            match input {
                Inputs::Delete => {
                    for (entity, player_id) in players.iter() {
                        if player_id.0 == client_id {
                            if let Some(mut entity_mut) = commands.get_entity(entity) {
                                // we need to use this special function to despawn prediction entity
                                // the reason is that we actually keep the entity around for a while,
                                // in case we need to re-store it for rollback
                                entity_mut.prediction_despawn();
                                debug!("Despawning the predicted/pre-predicted player because we received player action!");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

// Adjust the movement of the cursor entity based on the mouse position
fn cursor_movement(
    connection: Res<ClientConnection>,
    window_query: Query<&Window>,
    mut cursor_query: Query<
        (&mut CursorPosition, &PlayerId),
        Or<((Without<Confirmed>, Without<Interpolated>),)>,
    >,
) {
    let client_id = connection.id();
    for (mut cursor_position, player_id) in cursor_query.iter_mut() {
        if player_id.0 != client_id {
            return;
        }
        if let Ok(window) = window_query.get_single() {
            if let Some(mouse_position) = window_relative_mouse_position(window) {
                // only update the cursor if it's changed
                cursor_position.set_if_neq(CursorPosition(mouse_position));
            }
        }
    }
}

// Get the cursor position relative to the window
fn window_relative_mouse_position(window: &Window) -> Option<Vec2> {
    let Some(cursor_pos) = window.cursor_position() else {
        return None;
    };

    Some(Vec2::new(
        cursor_pos.x - (window.width() / 2.0),
        (cursor_pos.y - (window.height() / 2.0)) * -1.0,
    ))
}

// System to receive messages on the client
pub(crate) fn receive_message(mut reader: EventReader<ClientMessageEvent<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
    }
}

/// Send messages from server to clients
pub(crate) fn send_message(
    mut client: ResMut<ConnectionManager>,
    input: Res<ButtonInput<KeyCode>>,
) {
    if input.pressed(KeyCode::KeyM) {
        let message = Message1(5);
        info!("Send message: {:?}", message);
        // the message will be re-broadcasted by the server to all clients
        client
            .send_message_to_target::<Channel1, Message1>(&Message1(5), NetworkTarget::All)
            .unwrap_or_else(|e| {
                error!("Failed to send message: {:?}", e);
            });
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
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
