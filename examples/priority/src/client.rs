use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // All player entities are controlled by inputs, so we need to add the InputMap component
        // to predicted players
        app.add_systems(Update, add_input_map);
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);
    }
}

/// Add the Leafwing InputMap component to the predicted player entity
pub(crate) fn add_input_map(
    mut commands: Commands,
    predicted_players: Query<Entity, (Added<PlayerId>, With<Predicted>)>,
) {
    for player_entity in predicted_players.iter() {
        commands
            .entity(player_entity)
            .insert(InputMap::<Inputs>::new([
                (Inputs::Right, KeyCode::ArrowRight),
                (Inputs::Right, KeyCode::KeyD),
                (Inputs::Left, KeyCode::ArrowLeft),
                (Inputs::Left, KeyCode::KeyA),
                (Inputs::Up, KeyCode::ArrowUp),
                (Inputs::Up, KeyCode::KeyW),
                (Inputs::Down, KeyCode::ArrowDown),
                (Inputs::Down, KeyCode::KeyS),
            ]));
    }
}

/// The client input only gets applied to predicted entities that we own
fn player_movement(mut query: Query<(&mut Position, &ActionState<Inputs>), With<Predicted>>) {
    for (position, action_state) in query.iter_mut() {
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared::shared_movement_behaviour(position, action_state);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: Trigger<OnAdd, PlayerId>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.target();
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
