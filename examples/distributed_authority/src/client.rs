//! The client plugin.
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use crate::automation::AutomationClientPlugin;
use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use lightyear::input::client::InputSystems;
use lightyear::input::native::prelude::{ActionState, InputMarker};
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
        app.add_observer(handle_ball);
    }
}

/// When a Ball entity gets replicated to us from the server, add the Replicate component
/// on the client so that we can replicate updates to the server if we get authority
/// over the ball
pub(crate) fn handle_ball(trigger: On<Add, BallMarker>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert((Replicate::to_server(), Name::new("Ball")));
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    query.iter_mut().for_each(|mut action_state| {
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
        action_state.0 = Inputs::Direction(direction);
    });
}

fn player_movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        shared::shared_movement_behaviour(position, input);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, PlayerId>,
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

/// Add the local input marker once ownership is known.
pub(crate) fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<Option<&ControlledBy>, (With<PlayerId>, Without<InputMarker<Inputs>>)>,
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
    commands
        .entity(entity)
        .insert(InputMarker::<Inputs>::default());
}
