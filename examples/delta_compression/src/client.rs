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
use core::time::Duration;

use lightyear::client::input::InputSystemSet;
use lightyear::inputs::native::{ActionState, InputMarker};
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Inputs have to be buffered in the FixedPreUpdate schedule
        app.add_systems(
            FixedPreUpdate,
            // Use new InputSystemSet path
            buffer_input.in_set(input::InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (
                receive_message1,
                receive_entity_spawn,
                receive_entity_despawn,
                receive_player_id_insert,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
pub(crate) fn buffer_input(
    // Use new ActionState and InputManager paths
    mut query: Query<&mut ActionState<Inputs>, With<InputManager<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    query.iter_mut().for_each(|mut action_state| {
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
        // Use the set() method for ActionState
        action_state.set(input);
        // action_state.value = input;
    });
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        // Use the pressed() method to check if an input is active
        if input.pressed(&Inputs::Direction(Direction {
            // Check for any direction input
            up: true,
            down: false,
            left: false,
            right: false,
        })) || input.pressed(&Inputs::Direction(Direction {
            up: false,
            down: true,
            left: false,
            right: false,
        })) || input.pressed(&Inputs::Direction(Direction {
            up: false,
            down: false,
            left: true,
            right: false,
        })) || input.pressed(&Inputs::Direction(Direction {
            up: false,
            down: false,
            left: false,
            right: true,
        })) {
            // Retrieve the actual input value if needed for the behavior function
            // Assuming shared_movement_behaviour needs the specific direction
            if let Some(inputs) = input.current_value() {
                // Use current_value() to get the Option<Inputs>
                shared::shared_movement_behaviour(position, inputs);
            }
        }
        // if let Some(inputs) = &input.value { // Old check using private field
        //     shared::shared_movement_behaviour(position, inputs);
        // }
    }
}

/// System to receive messages on the client
pub(crate) fn receive_message1(mut reader: EventReader<ReceiveMessage<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
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

/// Example system to handle ComponentInsertEvent events
pub(crate) fn receive_player_id_insert(mut reader: EventReader<ComponentInsertEvent<PlayerId>>) {
    for event in reader.read() {
        info!(
            "Received component PlayerId insert for entity: {:?}",
            event.entity()
        );
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    mut predicted: Query<(Entity, &mut PlayerColor), Added<Predicted>>,
    mut commands: Commands,
) {
    for (entity, mut color) in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        commands
            .entity(entity)
            // Use new InputManager path
            .insert(InputManager::<Inputs>::default());
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
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
