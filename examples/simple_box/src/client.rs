//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;
use bevy::app::PluginGroupBuilder;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use lightyear::prelude::client::input::*;
use lightyear::prelude::client::*;
use lightyear::prelude::input::native::*;
use lightyear::prelude::input::*;
use lightyear::prelude::*;
use lightyear_examples_common::client::ExampleClient;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            // Inputs have to be buffered in the WriteClientInputs set
            buffer_input.in_set(InputSet::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, player_movement);

        app.add_systems(
            Update,
            (
                receive_message1,
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
        action_state.value = input;
    }
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    timeline: Single<&LocalTimeline>,
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    let tick = timeline.tick();
    for (position, input) in position_query.iter_mut() {
        if let Some(input) = &input.value {
            // info!(?tick, ?position, ?input, "client");
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared::shared_movement_behaviour(position, input);
        }
    }
}

/// System to receive messages on the client
pub(crate) fn receive_message1(mut receiver: Single<&mut MessageReceiver<Message1>>) {
    for message in receiver.receive() {
        info!("Received message: {:?}", message);
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
        warn!("Add InputMarker to entity: {:?}", entity);
        commands
            .entity(entity)
            .insert((InputMarker::<Inputs>::default(),));
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
