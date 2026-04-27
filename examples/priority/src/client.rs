use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
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
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
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

/// Add local input bindings once ownership is known.
pub(crate) fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<Option<&ControlledBy>, (With<PlayerId>, Without<InputMap<Inputs>>)>,
    clients: Query<(), With<Client>>,
) {
    let entity = trigger.entity;
    let Ok(controlled_by) = players.get(entity) else {
        return;
    };
    if let Some(controlled_by) = controlled_by {
        if clients.get(controlled_by.owner).is_err() {
            return;
        }
    }
    commands.entity(entity).insert(InputMap::<Inputs>::new([
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
