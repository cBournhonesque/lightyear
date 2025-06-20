use bevy::prelude::*;
use core::time::Duration;
use lightyear::input::client::InputSet;
use lightyear::input::native::prelude::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{color_from_id, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(spawn_local_cursor);
        // Inputs need to be buffered in the `FixedPreUpdate` schedule
        app.add_systems(
            FixedPreUpdate,
            write_inputs.in_set(InputSet::WriteClientInputs),
        );
        // all actions related-system that can be rolled back should be in the `FixedUpdate` schedule
        app.add_systems(FixedUpdate, (player_movement, delete_player));
        app.add_systems(Update, (cursor_movement, spawn_player));
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

/// Spawn a cursor that is replicated to the server when the client connects
pub(crate) fn spawn_local_cursor(
    trigger: Trigger<OnAdd, Connected>,
    client: Query<&LocalId, With<Client>>,
    mut commands: Commands,
) {
    if let Ok(client) = client.get(trigger.target()) {
        let client_id = client.0;
        // spawn a local cursor which will be replicated to the server
        let id = commands
            .spawn((
                PlayerId(client_id),
                CursorPosition(Vec2::ZERO),
                PlayerColor(color_from_id(client_id)),
                Replicate::to_server(),
                Name::from("Cursor"),
                // TODO: maybe add Interpolation so that the server interpolates the cursor updates?
            ))
            .id();
        info!("Spawning local cursor {id:?} for client: {}", client_id);
    }
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn write_inputs(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
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
        action_state.value = input;
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
    client: Single<&LocalId, With<Client>>,
    players: Query<&PlayerId, With<PlayerPosition>>,
) {
    if keypress.just_pressed(KeyCode::Space) {
        let client_id = client.into_inner().0;
        // do not spawn a new player if we already have one
        for player_id in players.iter() {
            if player_id.0 == client_id {
                return;
            }
        }
        info!(
            "Spawning client-owned player entity for client: {}",
            client_id
        );
        commands.spawn((
            Name::from("Player"),
            PlayerId(client_id),
            PlayerPosition(Vec2::ZERO),
            PlayerColor(color_from_id(client_id)),
            Replicate::to_server(),
            // IMPORTANT: this lets the server know that the entity is pre-predicted
            // when the server replicates this entity; we will get a Confirmed entity
            // which will use this entity as the Predicted version
            PrePredicted::default(),
        ));
    }
}

// TODO: This doesn't work properly because we when we despawn the entity here, it gets PredictionDisabled
//  so it doesn't appear in the input plugin's queries.
//  I'm waiting for bevy 0.17 and the 'Allows' filter to fix this properly, by adding 'Allows<PredictionDisabled>'
//  filters in the input plugin's queries
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
            if let Ok(mut entity_mut) = commands.get_entity(entity) {
                // we need to use this special function to despawn prediction entity
                // the reason is that we actually keep the entity around for a while,
                // in case we need to re-store it for rollback
                entity_mut.prediction_despawn();
                info!(
                    "Despawning the predicted/pre-predicted player because we received player action!"
                );
            }
        }
    }
}

// Adjust the movement of the cursor entity based on the mouse position
fn cursor_movement(
    client: Single<&LocalId, (With<Connected>, With<Client>)>,
    window_query: Query<&Window>,
    mut cursor_query: Query<
        (&mut CursorPosition, &PlayerId),
        // Query the client-authoritative cursor
        (Without<Confirmed>, Without<Interpolated>),
    >,
) {
    let client_id = client.into_inner().0;
    for (mut cursor_position, player_id) in cursor_query.iter_mut() {
        if player_id.0 != client_id {
            // This entity is replicated from another client, skip
            continue;
        }
        if let Ok(window) = window_query.single() {
            if let Some(mouse_position) = window_relative_mouse_position(window) {
                // only update the cursor if it's changed
                cursor_position.set_if_neq(CursorPosition(mouse_position));
            }
        }
    }
}

// Get the cursor position relative to the window
fn window_relative_mouse_position(window: &Window) -> Option<Vec2> {
    let cursor_pos = window.cursor_position()?;

    Some(Vec2::new(
        cursor_pos.x - (window.width() / 2.0),
        (cursor_pos.y - (window.height() / 2.0)) * -1.0,
    ))
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: Trigger<OnAdd, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, (With<Predicted>, With<PlayerId>)>,
    mut commands: Commands,
) {
    let entity = trigger.target();
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        warn!("Add InputMarker to entity: {:?}", entity);
        commands
            .entity(entity)
            .insert(InputMarker::<Inputs>::default());
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    trigger: Trigger<OnAdd, PlayerColor>,
    mut interpolated: Query<&mut PlayerColor, With<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
