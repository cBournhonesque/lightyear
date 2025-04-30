use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
// Assuming shared movement logic exists

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // All player entities are controlled by inputs, so we need to add the InputMap component
        // to predicted players
        app.add_systems(Update, add_input_map);
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (handle_predicted_spawn, handle_interpolated_spawn),
        );
    }
}



/// Add the Leafwing InputMap component to the predicted player entity
pub(crate) fn add_input_map(
    mut commands: Commands,
    predicted_players: Query<Entity, (Added<PlayerId>, With<Predicted>)>,
) {
    for player_entity in predicted_players.iter() {
        commands.entity(player_entity).insert((
            PlayerBundle::get_input_map(),
            ActionState::<Inputs>::default(),
        ));
    }
}

/// The client input only gets applied to predicted entities that we own
fn player_movement(
    mut query: Query<(&mut Position, &ActionState<Inputs>), With<Predicted>>,
) {
    for (position, action_state) in query.iter_mut() {
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared::shared_movement_behaviour(position, action_state);
    }
}


/// When the predicted copy of the client-owned entity is spawned
/// - assign it a different saturation
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4, // TODO: use component?
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// When the interpolated copy of the client-owned entity is spawned
/// - assign it a different saturation
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
