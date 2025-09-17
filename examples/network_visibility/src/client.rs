use bevy::prelude::*;
use lightyear::input::client::InputSet;
use lightyear::input::native::prelude::{ActionState, InputMarker};
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::shared_movement_behaviour;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            // Inputs have to be buffered in the WriteClientInputs set
            buffer_input.in_set(InputSet::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);
    }
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        let mut direction = Inputs::default();
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
        action_state.0 = direction;
    }
}

pub(crate) fn movement(
    // TODO: maybe make prediction mode a separate component!!!
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), With<Predicted>>,
) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
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
