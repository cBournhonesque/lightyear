use bevy::prelude::*;
use core::time::Duration;
// Use new ActionState + InputManager paths
use lightyear::prelude::client::{ActionState, ClientConnection, InputManager, Replicate};
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{color_from_id, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Removed init system
        // app.add_systems(Startup, init);
        // Run spawn_local_cursor once when ClientConnection resource is added
        app.add_systems(
            Update,
            spawn_local_cursor.run_if(resource_added::<ClientConnection>),
        );
        // app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive)); // Removed old handler
        // Inputs need to be buffered in the `FixedPreUpdate` schedule
        app.add_systems(
            FixedPreUpdate,
            // Use new InputSystemSet path
            buffer_input.in_set(input::InputSystemSet::BufferInputs),
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

// Removed init system
// pub(crate) fn init(mut commands: Commands) {
//     commands.connect_client();
// }

/// Spawn the local cursor when the client connects
// Renamed from handle_connection, uses ClientConnection resource
pub(crate) fn spawn_local_cursor(
    mut commands: Commands,
    connection: Res<ClientConnection>, // Get ClientConnection resource
) {
    let client_id = connection.id(); // Get PeerId
    info!("Spawning local cursor for client: {}", client_id);
    // spawn a local cursor which will be replicated to other clients, but remain client-authoritative.
    commands.spawn((
        CursorBundle::new(
            client_id,
            Vec2::ZERO,
            color_from_id(client_id), // color_from_id now takes PeerId
        ),
        // Add Replicate component for client-driven replication.
        // Use default() as the configuration seems simplified.
        Replicate::default(),
        // Replicate {
        //     replication_target: NetworkTarget::All, // Replicate to server and other clients
        //     sync: SyncTarget {
        //         prediction: NetworkTarget::None, // No prediction for cursor
        //         interpolation: NetworkTarget::All, // Interpolate cursor on other clients
        //     },
        //     controlled_by: ControlledBy::Client, // Client controls this entity
        //     ..default()
        // },
    ));
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(
    // Use new ActionState and InputManager paths
    mut query: Query<&mut ActionState<Inputs>, With<InputManager<Inputs>>>,
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
        if keypress.pressed(KeyCode::KeyK) {
            input = Some(Inputs::Delete);
        }
        // Use the set() method for ActionState
        action_state.set(input);
        // action_state.value = input;
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
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

/// Spawn a client-owned player entity when the space command is pressed
fn spawn_player(
    mut commands: Commands,
    keypress: Res<ButtonInput<KeyCode>>,
    // Use LocalPlayerId resource
    local_player_id: Option<Res<LocalPlayerId>>,
    players: Query<&PlayerId, With<PlayerPosition>>,
) {
    let Some(local_player_id) = local_player_id else {
        warn!("LocalPlayerId resource not found, cannot spawn player yet.");
        return;
    };
    let client_id = local_player_id.0; // Get PeerId

    // do not spawn a new player if we already have one
    for player_id in players.iter() {
        if player_id.0 == client_id {
            return;
        }
    }
    if keypress.just_pressed(KeyCode::Space) {
        info!("Spawning client-owned player entity for client: {}", client_id);
        commands.spawn((
            PlayerBundle::new(client_id, Vec2::ZERO),
            // add InputManager so we can attach inputs to this entity
            InputManager::<Inputs>::default(),
            // Add Replicate component for client-driven replication.
            // Use default() as the configuration seems simplified.
            Replicate::default(),
            // Replicate {
            //     replication_target: NetworkTarget::All, // Replicate to server and other clients
            //     sync: SyncTarget {
            //         prediction: NetworkTarget::All, // Predict player on all clients
            //         interpolation: NetworkTarget::None, // No interpolation for predicted entities
            //     },
            //     controlled_by: ControlledBy::Client, // Client controls this entity
            //     ..default()
            // },
            // PrePredicted is automatically added by client::Replicate
            // PrePredicted::default(),
        ));
    }
}

/// Delete the predicted player when the space command is pressed
fn delete_player(
    mut commands: Commands,
    players: Query<
        (Entity, &ActionState<Inputs>),
        (
            With<PlayerPosition>,
            Without<Confirmed>,
            Without<Interpolated>,
        ),
    >,
) {
    for (entity, inputs) in players.iter() {
        if inputs.value.as_ref().is_some_and(|v| v == &Inputs::Delete) {
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

// Adjust the movement of the cursor entity based on the mouse position
fn cursor_movement(
    // Use LocalPlayerId resource
    local_player_id: Option<Res<LocalPlayerId>>,
    window_query: Query<&Window>,
    mut cursor_query: Query<
        (&mut CursorPosition, &PlayerId),
        // Query the client-authoritative cursor
        (With<Replicate>, Without<Confirmed>, Without<Interpolated>),
    >,
) {
    let Some(local_player_id) = local_player_id else {
        warn!("LocalPlayerId resource not found, cannot move cursor yet.");
        return;
    };
    let client_id = local_player_id.0; // Get PeerId

    for (mut cursor_position, player_id) in cursor_query.iter_mut() {
        if player_id.0 != client_id {
            // This entity is replicated from another client, skip
            continue;
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
pub(crate) fn receive_message(mut reader: EventReader<ReceiveMessage<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
    }
}

/// Send messages from client to server
pub(crate) fn send_message(
    // Use ClientConnection resource
    client: Res<ClientConnection>,
    input: Res<ButtonInput<KeyCode>>,
) {
    if input.pressed(KeyCode::KeyM) {
        let message = Message1(5);
        info!("Send message: {:?}", message);
        // Send to server
        client.send_message::<Channel1, Message1>(message)
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
