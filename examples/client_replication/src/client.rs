use bevy::prelude::*;
use core::time::Duration;
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSet;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::color_from_id;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_connect);
        app.add_observer(on_admin_context);
        app.add_systems(Update, cursor_movement);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

/// Spawn a cursor that is replicated to the server when the client connects
/// Add an ActionState component on the Client entity to send inputs to the server
pub(crate) fn on_connect(
    trigger: On<Add, Connected>,
    client: Query<&LocalId, With<Client>>,
    mut commands: Commands,
) {
    if let Ok(client) = client.get(trigger.entity) {
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

// Adjust the movement of the cursor entity based on the mouse position
fn cursor_movement(
    client: Single<&LocalId, (With<Connected>, With<Client>)>,
    window_query: Query<&Window>,
    mut cursor_query: Single<&mut CursorPosition, With<Replicate>>,
) {
    let client_id = client.into_inner().0;
    let mut cursor_position = cursor_query.into_inner();
    if let Ok(window) = window_query.single() {
        if let Some(mouse_position) = window_relative_mouse_position(window) {
            // only update the cursor if it's changed
            cursor_position.set_if_neq(CursorPosition(mouse_position));
        }
    }
}

// Get the cursor position relative to the window
fn window_relative_mouse_position(window: &Window) -> Option<Vec2> {
    let cursor_pos = window.cursor_position()?;

    Some(Vec2::new(
        cursor_pos.x - (window.width() / 2.0),
        -(cursor_pos.y - (window.height() / 2.0)),
    ))
}

fn on_admin_context(trigger: On<Add, Admin>, mut commands: Commands) {
    commands.spawn((
        ActionOf::<Admin>::new(trigger.entity),
        Action::<SpawnPlayer>::new(),
        bindings![KeyCode::Space,],
    ));
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - add InputActions to move and despawn the entity
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, (With<Predicted>, With<PlayerId>)>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, PlayerColor>,
    mut interpolated: Query<&mut PlayerColor, With<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
